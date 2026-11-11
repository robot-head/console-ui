import SwiftUI
import AppKit
import Charts
import Darwin
import UniformTypeIdentifiers

struct Timing: Decodable { let total, busy, scheduled, idle, polls: UInt64 }
struct TaskRow: Decodable, Identifiable { let id: UInt64; let name, target, location, fields, state: String; let blocking: Bool; let timing: Timing; let wakes, selfWakes, wakerClones, wakerDrops: UInt64; let lastWakeAgo, sizeBytes, originalSizeBytes: UInt64?; let warnings: [String] }
struct ResourceRow: Decodable, Identifiable { let id: UInt64; let kind, concreteType, target, location, attributes: String; let `internal`: Bool; let parent: UInt64?; let total: UInt64; let completed: Bool }
struct OperationRow: Decodable, Identifiable { let id, resourceId: UInt64; let taskId: UInt64?; let taskName, source, attributes: String; let timing: Timing; let completed: Bool; let ready: Bool? }
struct HistogramBin: Decodable, Identifiable { let upperNs, count: UInt64; var id: UInt64 { upperNs } }
struct Distribution: Decodable { let count, min, max: UInt64; let percentiles: [UInt64]; let bins: [HistogramBin]; let highOutliers: UInt64; let highestOutlier: UInt64?; let error: String? }
struct TaskDetails: Decodable { let id: UInt64; let poll, scheduled: Distribution }
struct Snapshot: Decodable { let now, updates, droppedEvents: UInt64; let tasks: [TaskRow]; let resources: [ResourceRow]; let asyncOps: [OperationRow]; let details: [String: TaskDetails] }
struct TargetTab: Decodable, Identifiable { let id: UInt64; let name, target, status: String; let recording: String? }
struct Selection: Decodable { let kind: String; let id, session: UInt64 }
struct SessionInfo: Decodable, Identifiable { let index: Int; let start, end, updates: UInt64; let target: String?; let incomplete: Bool; var id: Int { index } }
struct Bookmark: Decodable, Identifiable { let id: UInt64; let name, note: String; let session: Int; let cursor: UInt64 }
struct SavedView: Decodable, Identifiable { let name: String; var id: String { name } }
struct Profile: Decodable, Identifiable { let name, target: String; var id: String { name } }
struct Preset: Decodable, Identifiable { let name, expression: String; var id: String { name } }
struct EditorMapping: Decodable { let remoteRoot, localRoot: String }
struct EditorPreferences: Decodable { let program: String; let arguments:[String]; let mappings:[EditorMapping] }
struct Settings: Decodable {let palette:String;let warnings:[String]}
struct Preferences: Decodable { let settings:Settings; let editor:EditorPreferences; let profiles: [Profile]; let recentTargets: [String]; let filterPresets: [Preset] }
struct Trend: Decodable, Identifiable { let now, busyNs, scheduledNs: UInt64; let pollsPerSecond: Double; let gap: Bool; var id: UInt64 { now } }
struct Evidence: Decodable, Identifiable { let task: UInt64; let lint, measured, threshold, explanation, source: String; var id: String { "\(task):\(lint)" } }
struct GraphNode: Decodable, Identifiable { let key, label, kind: String; var id: String { key } }
struct GraphEdge: Decodable, Identifiable { let from, to, label: String; let operation: UInt64?; var id: String { "\(from):\(to):\(operation ?? 0)" } }
struct Graph: Decodable { let nodes: [GraphNode]; let edges: [GraphEdge]; let truncated: Bool }
struct ComparisonRow: Decodable, Identifiable { let kind:String; let key, name, status: String; let baseline, current: UInt64?; let busyPerSecond, scheduledPerSecond, pollsPerSecond: [Double]; let regression: Bool; var id: String { key } }
struct Comparison: Decodable { let rows: [ComparisonRow]; let notes: [String] }
struct ExportPreview: Decodable { let target:String?; let snapshot:Snapshot; let sections:[String] }
struct HistogramPolicy: Decodable { let mode,query:String; let tasks:[UInt64];let maxStreams:Int;let rotateSeconds:UInt64 }
struct RuntimeView: Decodable { let histogramPolicy:HistogramPolicy;let histogramNotice:String?; let inspection:Snapshot; let lookupTasks:[TaskRow]; let exportPreview:ExportPreview?; let reportSections, projectPresets:[String]; let name: String; let active: UInt64; let tabs: [TargetTab]; let status: String; let paused: Bool; let target: String; let recording: String?; let session: UInt64; let snapshot: Snapshot; let query: String; let selection: Selection?; let trends: [Trend]; let evidence: [Evidence]; let graph: Graph; let sessions: [SessionInfo]; let cursor: UInt64?; let playing: Bool; let speed: Double; let bookmarks: [Bookmark]; let views: [SavedView]; let preferences: Preferences; let comparison: Comparison?; let notice: String?; let pending: Int }

@MainActor final class RuntimeModel: ObservableObject {
    @Published var state: RuntimeView?
    @Published var error: String?
    @Published var lookup = false
    private var handle: OpaquePointer?
    private var timer: Timer?
    private var lastNotice: String?
    private let decoder: JSONDecoder = { let d = JSONDecoder(); d.keyDecodingStrategy = .convertFromSnakeCase; return d }()
    private var smokeStep = 0
    private let started = Date()
    let smoke = ProcessInfo.processInfo.arguments.contains("--smoke")
    init() {
        guard console_ui_abi_version() == 1 else { error = "Unsupported native bridge version"; return }
        handle = console_ui_create()
        guard handle != nil else { error = String(cString: console_ui_last_error()); return }
        refresh()
        timer = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in Task { @MainActor in self?.tick() } }
        RunLoop.main.add(timer!, forMode: .common)
    }
    func shutdown() { timer?.invalidate(); timer = nil; if let h = handle { console_ui_destroy(h); handle = nil } }
    private func consume(_ pointer: UnsafeMutablePointer<CChar>?) -> Data? {
        guard let pointer else { error = "The diagnostic controller returned no result"; return nil }
        defer { console_ui_string_free(pointer) }
        return String(cString: pointer).data(using: .utf8)
    }
    func send(_ action: String, _ fields: [String: Any] = [:]) {
        guard let handle else { return }
        do {
            var value = fields; value["action"] = action
            let data = try JSONSerialization.data(withJSONObject: value)
            let encoded = String(decoding: data, as: UTF8.self)
            if let response = encoded.withCString({ consume(console_ui_dispatch(handle, $0)) }),
               let reply = try JSONSerialization.jsonObject(with: response) as? [String: Any], reply["ok"] as? Bool != true {
                error = reply["error"] as? String ?? "Diagnostic action failed"
            }
            refresh()
        } catch { self.error = error.localizedDescription }
    }
    func refresh() {
        guard let handle, let data = consume(console_ui_view(handle)) else { return }
        do { let next = try decoder.decode(RuntimeView.self, from: data); state = next
            if let notice = next.notice, notice != lastNotice { lastNotice = notice }
            if next.notice == nil { lastNotice = nil }
        } catch { self.error = "Read diagnostic state: \(error.localizedDescription)" }
    }
    private func tick() {
        send("tick")
        guard smoke, let state else { return }
        switch smokeStep {
        case 0 where !state.snapshot.tasks.isEmpty && !state.snapshot.resources.isEmpty:
            let task = state.snapshot.tasks.first(where: { $0.name == "worker-0" }) ?? state.snapshot.tasks[0]
            send("select", ["kind": "task", "id": task.id]); smokeStep = 1
        case 1 where !state.snapshot.details.isEmpty: send("pause"); smokeStep = 2
        case 2 where state.paused: send("pause"); smokeStep = 3
        case 3 where !state.paused: do { try smokeScreenshot(); print("NATIVE_MACOS_SMOKE_OK"); shutdown(); NSApplication.shared.terminate(nil) } catch { fputs("Capture native screenshot: \(error)\n",stderr); shutdown(); exit(1) }
        default: break
        }
        if Date().timeIntervalSince(started) > 40 { fputs("Native macOS smoke timed out\n", stderr); shutdown(); exit(1) }
    }
    func chooseFile(save: Bool = false, extensions: [String] = ["tcui"], action: @escaping (String) -> Void) {
        let originalTab=state?.active
        let guarded: (String)->Void = { [weak self] path in guard self?.state?.active == originalTab else { self?.error="The active investigation changed; choose the file again"; return }; action(path) }
        if save { let panel = NSSavePanel(); panel.allowedContentTypes=extensions.compactMap { UTType(filenameExtension:$0) }; panel.canCreateDirectories = true; panel.nameFieldStringValue = "capture.\(extensions[0])"; panel.begin { response in if response == .OK, let url = panel.url { guarded(url.path) } } }
        else { let panel = NSOpenPanel(); panel.allowedContentTypes=extensions.compactMap { UTType(filenameExtension:$0) }; panel.canChooseDirectories = false; panel.allowsMultipleSelection = false; panel.begin { response in if response == .OK, let url = panel.url { guarded(url.path) } } }
    }
    func select(_ kind: String, _ id: UInt64) { send("select_in_session", ["kind": kind, "id": id, "session":state?.session ?? 0]) }
}

enum Section: String, CaseIterable, Identifiable, Hashable { case tasks = "Tasks", resources = "Resources", operations = "Async operations", analysis = "Analysis", recording = "Recording", compare = "Compare", sharing = "Export & share", connections = "Connections", workspace = "Workspace"; var id: String { rawValue } }
struct ContentView: View {
    @ObservedObject var model: RuntimeModel
    @State private var section: Section? = .tasks
    @State private var endpoint = "http://127.0.0.1:6669"
    @State private var query = ""
    var body: some View {
        NavigationSplitView {
            List(Section.allCases, selection: $section) { s in Text(s.rawValue).tag(s) }
            .navigationTitle("Console UI")
        } content: {
            VStack(alignment: .leading, spacing: 12) {
                HStack { Text(model.state?.status.uppercased() ?? "STARTING").font(.caption.bold()); if model.state?.paused == true { Text("PAUSED").foregroundStyle(.orange) }; Spacer(); if let s = model.state { Text("\(s.snapshot.tasks.count) tasks · \(s.snapshot.resources.count) resources") } }
                if let state = model.state {
                    ScrollView(.horizontal) {
                        HStack {
                            ForEach(state.tabs) { tab in
                                HStack {
                                    Button(tab.name + (tab.recording == nil ? "" : " ● REC")) { model.send("select_tab", ["id":tab.id]) }.buttonStyle(.bordered)
                                    if state.tabs.count > 1 {
                                        Button { model.send("close_tab", ["id":tab.id]) } label: { Image(systemName:"xmark") }.help("Close \(tab.name)")
                                    }
                                }
                            }
                        }
                    }
                    if let notice = state.notice { Text(notice).foregroundStyle(.secondary).textSelection(.enabled) }
                    if state.pending > 0 { ProgressView("Processing recording or export…").controlSize(.small) }
                }
                HStack { TextField("Telemetry endpoint", text: $endpoint).onSubmit { model.send("connect", ["target": endpoint]) }; Button("Connect") { model.send("connect", ["target": endpoint]) }; Button("Disconnect") { model.send("disconnect") }; Button(model.state?.paused == true ? "Resume" : "Pause") { model.send("pause") }.disabled(model.state?.status != "Connected") }
                HStack { TextField("Filter: busy>5ms !state:completed | warning:self-wakes", text: $query).onSubmit { applyFilter() }; Button("Apply") { applyFilter() }; Button("Clear") { query = ""; applyFilter() }; Button("Find task") { model.lookup = true }.keyboardShortcut("k", modifiers: .command) }
                Group {
                    switch section ?? .tasks {
                    case .tasks: TaskTable(model: model)
                    case .resources: ResourceTable(model: model)
                    case .operations: OperationTable(model: model)
                    case .analysis: AnalysisPanel(model: model)
                    case .recording: RecordingPanel(model: model)
                    case .compare: ComparePanel(model: model)
                    case .sharing: SharingPanel(model: model)
                    case .connections: ConnectionsPanel(model: model)
                    case .workspace: WorkspacePanel(model: model)
                    }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }.padding(16).frame(minWidth: 650)
        } detail: {
            Inspector(model: model).frame(minWidth: 320, idealWidth: 400)
        }
        .preferredColorScheme(model.state?.preferences.settings.palette == "dark" ? .dark : model.state?.preferences.settings.palette == "light" ? .light : nil)
        .onChange(of: model.state?.active) { _, _ in endpoint = model.state?.target ?? endpoint; query = model.state?.query ?? "" }
        .onChange(of: model.state?.target) { _, value in endpoint=value ?? endpoint }
        .onChange(of: model.state?.query) { _, value in query = value ?? "" }
        .alert("Console UI", isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })) { Button("OK") { model.error = nil } } message: { Text(model.error ?? "") }
        .sheet(isPresented: $model.lookup) { LookupPanel(model: model) }
        .onOpenURL { url in model.send("open_recording", ["path": url.path, "recover": false]); section = .recording }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.willTerminateNotification)) { _ in model.shutdown() }
    }
    private func applyFilter() { model.send("filter", ["expression": query]) }
}

struct TaskTable: View {
    @ObservedObject var model: RuntimeModel
    @State private var order = [KeyPathComparator(\TaskRow.name)]
    private var selected: Binding<UInt64?> { Binding(get: { model.state?.selection?.kind == "task" ? model.state?.selection?.id : nil }, set: { if let id = $0 { model.select("task", id) } }) }
    var body: some View { Table((model.state?.snapshot.tasks ?? []).sorted(using: order), selection: selected, sortOrder: $order) {
        TableColumn("ID", value: \.id) { Text(String($0.id)) }.width(min: 50, ideal: 70)
        TableColumn("Name", value: \.name).width(min: 130, ideal: 220)
        TableColumn("State", value: \.state).width(min: 80, ideal: 100)
        TableColumn("Busy", value: \.timing.busy) { Text(ns($0.timing.busy)) }.width(min: 80, ideal: 100)
        TableColumn("Scheduled", value: \.timing.scheduled) { Text(ns($0.timing.scheduled)) }.width(min: 80, ideal: 100)
        TableColumn("Polls", value: \.timing.polls) { Text(String($0.timing.polls)) }.width(min: 60, ideal: 90)
        TableColumn("Source", value: \.location).width(min: 150, ideal: 250)
    }.contextMenu { Button("Open source") { model.send("open_source") } } }
}
struct ResourceTable: View {
    @ObservedObject var model: RuntimeModel
    @State private var order = [KeyPathComparator(\ResourceRow.total, order: .reverse)]
    private var selected: Binding<UInt64?> { Binding(get: { model.state?.selection?.kind == "resource" ? model.state?.selection?.id : nil }, set: { if let id = $0 { model.select("resource", id) } }) }
    var body: some View { Table((model.state?.snapshot.resources ?? []).sorted(using: order), selection: selected, sortOrder: $order) {
        TableColumn("ID", value: \.id) { Text(String($0.id)) }
        TableColumn("Type", value: \.concreteType)
        TableColumn("Kind", value: \.kind)
        TableColumn("Lifetime", value: \.total) { Text(ns($0.total)) }
        TableColumn("Visibility") { Text($0.internal ? "INT" : "PUB") }
        TableColumn("Source", value: \.location)
    } }
}
struct OperationTable: View {
    @ObservedObject var model: RuntimeModel
    var body: some View { Table(model.state?.snapshot.asyncOps ?? [], selection: Binding(get: { model.state?.selection?.kind == "operation" ? model.state?.selection?.id : nil }, set: { if let id = $0 { model.select("operation", id) } })) {
        TableColumn("ID") { Text(String($0.id)) }; TableColumn("Task") { Text("\($0.taskId ?? 0) \($0.taskName)") }; TableColumn("Source", value: \.source); TableColumn("Busy") { Text(ns($0.timing.busy)) }; TableColumn("Polls") { Text(String($0.timing.polls)) }; TableColumn("Attributes", value: \.attributes)
    } }
}
struct Inspector: View {
    @ObservedObject var model: RuntimeModel
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        if let state = model.state, let selected = state.selection {
            if selected.kind == "task", let t = state.inspection.tasks.first(where: { $0.id == selected.id }) {
                Text("\(t.name) / task \(t.id)").font(.title2)
                Text("\(t.state) · \(t.blocking ? "blocking" : "async")")
                ForEach(t.warnings, id: \.self) { Text("Warning: \($0)").foregroundStyle(.orange) }
                GroupBox("Task timing") { VStack(alignment: .leading) { metric("Total", ns(t.timing.total)); metric("Busy", "\(ns(t.timing.busy)) (\(percent(t.timing.busy,t.timing.total))%)"); metric("Scheduled", "\(ns(t.timing.scheduled)) (\(percent(t.timing.scheduled,t.timing.total))%)"); metric("Idle", ns(t.timing.idle)); metric("Polls", String(t.timing.polls)) } }
                GroupBox("Waker activity") { VStack(alignment: .leading) { metric("Active", String(t.wakerClones >= t.wakerDrops ? t.wakerClones-t.wakerDrops : 0)); metric("Clones / drops", "\(t.wakerClones) / \(t.wakerDrops)"); metric("Self wakes", "\(t.selfWakes) (\(percent(t.selfWakes,t.wakes))%)"); metric("Last woken", t.lastWakeAgo.map { ns($0) + " ago" } ?? "Never") } }
                if let details = state.inspection.details[String(t.id)] { HistogramPanel(name: "Poll times", histogram: details.poll); HistogramPanel(name: "Scheduled times", histogram: details.scheduled) } else { if state.status == "Offline" {Text("Histograms were not captured at this position. Missing samples are unknown.")} else {ProgressView("Waiting for task details…")} }
                Text(t.location).textSelection(.enabled); Text(t.fields).font(.system(.caption, design: .monospaced)).textSelection(.enabled); Button("Open source in editor") { model.send("open_source") }
            } else if selected.kind == "resource", let r = state.inspection.resources.first(where: { $0.id == selected.id }) {
                Text("\(r.concreteType) / resource \(r.id)").font(.title2); Text("\(r.kind) · \(r.internal ? "INT" : "PUB") · \(ns(r.total))"); Text(r.location).textSelection(.enabled); Text(r.attributes).textSelection(.enabled)
                ForEach(state.inspection.asyncOps.filter { $0.resourceId == r.id }) { op in GroupBox(op.source) { VStack(alignment: .leading) { Text("Task \(op.taskId ?? 0) \(op.taskName)"); Text("\(op.timing.polls) polls · \(ns(op.timing.busy)) busy · \(ns(op.timing.idle)) idle"); Text(op.attributes); if let id = op.taskId { Button("Inspect waiting task") { model.select("task", id) } } } } }
            } else if let op = state.inspection.asyncOps.first(where: { $0.id == selected.id }) { Text(op.source).font(.title2); Text(op.attributes); if let id = op.taskId { Button("Inspect operation's task") { model.select("task", id) } } }
        } else { Text("Select a task or resource to inspect timing, wakers, source fields, and latency distributions.").foregroundStyle(.secondary) }
    }.padding(16).frame(maxWidth: .infinity, alignment: .leading) } }
}
struct HistogramPanel: View {
    let name: String; let histogram: Distribution
    var body: some View { GroupBox(name) { VStack(alignment: .leading) {
        if let error = histogram.error { Text(error) } else { Text("\(histogram.count) samples · \(histogram.highOutliers) high outliers"); Text(zip(["p10","p25","p50","p75","p90","p95","p99"],histogram.percentiles).map { pair in "\(pair.0) \(ns(pair.1))" }.joined(separator: " · ")).font(.caption)
            Chart(histogram.bins) { bin in BarMark(x: .value("Bucket upper bound (ns)", String(bin.upperNs)), y: .value("Samples", bin.count)) }.frame(height: 150).chartXAxisLabel("Poll/scheduling duration bucket (ns)")
            if let high = histogram.highestOutlier { Text("Highest outlier: \(ns(high)). Percentiles cover in-range samples.").font(.caption) }
        }
    } } }
}
struct AnalysisPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var seconds = "60"
    @State private var wake = "50"
    @State private var never = "1s"
    @State private var future = "1024"
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        Text("Runtime analysis").font(.title)
        ForEach(["self-wakes","lost-waker","never-yielded","large-future","auto-boxed-future"],id:\.self) {lint in Toggle(lint,isOn:Binding(get:{model.state?.preferences.settings.warnings.contains(lint) ?? true},set:{model.send("lint_enabled",["lint":lint,"enabled":$0])}))}
        HStack { TextField("Window seconds", text: $seconds); Button("Apply window") { if let s = UInt64(seconds), s > 0, s <= UInt64.max/1_000_000_000 { model.send("window", ["nanoseconds": s*1_000_000_000]) } else { model.error = "Enter a positive number of seconds" } } }
        if let state = model.state {
            ForEach(["busy","scheduled","polls"],id:\.self) { metric in TrendChart(points:state.trends,metric:metric) }
            Text("Gaps and dropped events indicate incomplete evidence; task timing in the inspector is lifetime data.").font(.caption)
            HStack { TextField("Self-wake %", text: $wake); TextField("Never-yielded duration", text: $never); TextField("Future bytes", text: $future); Button("Apply thresholds") { if let percent = UInt8(wake), percent <= 100, let size = UInt64(future), let duration = durationNs(never), size > 0 { model.send("thresholds", ["thresholds": ["self_wake_percent": percent, "never_yielded_ns": duration, "large_future_bytes": size]]) } else { model.error = "Enter valid percentage, duration, and size thresholds" } } }
            ForEach(state.evidence) { e in GroupBox("Task \(e.task) · \(e.lint)") { VStack(alignment: .leading) { Text("Observed \(e.measured); threshold \(e.threshold)"); Text(e.explanation); Text(e.source).font(.caption); Button("Inspect task") { model.select("task", e.task) } } } }
            Text("Task/resource relationships").font(.headline)
            Text("Operation readiness does not establish lock ownership.").font(.caption)
            ForEach(state.graph.edges) { edge in HStack { Text("\(edge.from) → \(edge.to) · \(edge.label)"); Button("Inspect") { let parts = edge.from.split(separator: ":"); if parts.count == 2, let id = UInt64(parts[1]) { model.select(String(parts[0]), id) } } } }
            if state.graph.truncated { Text("Relationship view limited to 300 nodes. Select an entity to focus its neighbors.") }
        }
    }.padding() } }
}
struct RecordingPanel: View {
    @ObservedObject var model: RuntimeModel
    var body: some View {
        ScrollView { VStack(alignment:.leading,spacing:16) {
            Text("Recording and replay").font(.title)
            HStack {
                Button("Start recording…") { model.chooseFile(save:true) { model.send("start_recording",["path":$0]) } }
                Button("Stop recording") {model.send("stop_recording")}
                Button("Open recording…") {model.chooseFile {model.send("open_recording",["path":$0,"recover":false])}}
                Button("Recover partial capture…") {model.chooseFile {model.send("open_recording",["path":$0,"recover":true])}}
            }
            if let state=model.state, !state.sessions.isEmpty {
                ReplayControls(model:model,state:state)
                BookmarkControls(model:model)
            }
            HistogramControls(model:model)
        }.padding() }
    }
}
struct ReplayControls: View {
    @ObservedObject var model:RuntimeModel
    let state:RuntimeView
    @State private var offset=0.0
    @State private var dragging=false
    private var session:SessionInfo? {state.sessions.first {$0.index == max(0,Int(state.session)-1)}}
    private var index:Binding<Int> {Binding(get:{max(0,Int(state.session)-1)},set:{model.send("session",["index":$0])})}
    var body:some View {VStack(alignment:.leading,spacing:12) {
        Picker("Session",selection:index) {ForEach(state.sessions) {s in Text("Session \(s.index) · \(s.incomplete ? "incomplete" : "complete")").tag(s.index)}}
        if let session {
            Slider(value:$offset,in:0...max(0.001,Double(session.end-session.start)/1e9),onEditingChanged:{editing in dragging=editing;if !editing {model.send("seek",["cursor":session.start+UInt64(offset*1e9)])}})
            Text("Cursor \(state.cursor ?? session.start) ns · offset \(offset,specifier:"%.3f")s")
            Button(state.playing ? "Pause playback" : "Play recording") {model.send("playback")}
            HStack {ForEach([0.5,1.0,2.0,4.0],id:\.self) {speed in Button("\(speed,specifier:"%.1f")×") {model.send("speed",["speed":speed])}}}
        }
    }.onAppear {syncOffset()}.onChange(of:state.cursor) {_,_ in syncOffset()}}
    private func syncOffset() {if !dragging,let session,let cursor=state.cursor,cursor>=session.start {offset=Double(cursor-session.start)/1e9}}
}
struct BookmarkControls:View {
    @ObservedObject var model:RuntimeModel
    @State private var name=""
    @State private var note=""
    @State private var editing:UInt64?
    var body:some View {VStack(alignment:.leading,spacing:12) {
        TextField("Bookmark name",text:$name);TextField("Investigation note",text:$note)
        HStack {
            Button(editing == nil ? "Bookmark cursor" : "Save bookmark changes") {if let editing {model.send("edit_bookmark",["id":editing,"name":name,"note":note])} else {model.send("bookmark",["name":name,"note":note])}}
            Button("New bookmark") {editing=nil;name="";note=""}
        }
        ForEach(model.state?.bookmarks ?? []) {b in HStack {
            Text("\(b.name) · \(b.cursor) ns · \(b.note)")
            Button("Go") {model.send("go_bookmark",["id":b.id])}
            Button("Edit") {editing=b.id;name=b.name;note=b.note}
            Button("Delete") {model.send("delete_bookmark",["id":b.id])}
        }}
    }}
}
struct HistogramControls:View {
    @ObservedObject var model:RuntimeModel
    @State private var mode="inspected"
    @State private var ids=""
    @State private var query=""
    @State private var limit="4"
    @State private var rotation="30"
    var body:some View {GroupBox("Histogram capture policy") {VStack(alignment:.leading,spacing:12) {
        Picker("Mode",selection:$mode) {Text("Inspected task").tag("inspected");Text("Selected IDs").tag("selected");Text("Matching tasks").tag("matching")}
        TextField("Task IDs, comma separated",text:$ids);TextField("Matching task filter",text:$query)
        HStack {TextField("Maximum streams (1–32)",text:$limit);TextField("Rotation seconds",text:$rotation)}
        Button("Save capture policy") {apply()}
        Text(model.state?.histogramNotice ?? "Additional subscriptions run during recording. Missing histograms are unknown.").font(.caption)
    }}.onAppear {if let p=model.state?.histogramPolicy {mode=p.mode;ids=p.tasks.map(String.init).joined(separator:",");query=p.query;limit=String(p.maxStreams);rotation=String(p.rotateSeconds)}}}
    private func apply() {
        let values=ids.split(separator:",").map {$0.trimmingCharacters(in:.whitespaces)}
        let tasks=values.compactMap(UInt64.init)
        guard tasks.count==values.count,let maximum=Int(limit),(1...32).contains(maximum),let seconds=UInt64(rotation),seconds>0 else {model.error="Enter valid task IDs, stream limit, and rotation";return}
        model.send("histogram_policy",["policy":["mode":mode,"tasks":tasks,"query":query,"max_streams":maximum,"rotate_seconds":seconds]])
    }
}
struct ComparePanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var session = "0"
    @State private var relative = "10"
    @State private var absoluteMs = "1"
    @State private var polls = "100"
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        Text("Compare captures").font(.title); Text("Open the current capture, then select a baseline. Values use interval-normalized timing and polling.")
        HStack { TextField("Baseline session", text: $session); TextField("Relative %", text: $relative); TextField("Absolute ms", text: $absoluteMs); TextField("Polls/s increase", text: $polls) }
        Button("Choose baseline…") { if let index = Int(session), index >= 0, let percent = Double(relative), percent.isFinite, percent >= 0, let ms = Double(absoluteMs), ms.isFinite, ms >= 0, ms*1e6 < Double(UInt64.max), let polling = UInt64(polls) { model.chooseFile { model.send("compare", ["path": $0, "session": index, "policy": ["relative_percent": percent, "absolute_ns": UInt64(ms*1e6), "polls_increase": polling]]) } } else { model.error = "Enter valid session and regression thresholds" } }
        if let comparison = model.state?.comparison { ForEach(comparison.notes, id: \.self) { Text($0).font(.caption) }; ForEach(comparison.rows) { row in GroupBox("\(row.name) · \(row.status)") { VStack(alignment: .leading) { Text("Busy \(row.busyPerSecond[0], specifier: "%.2f") → \(row.busyPerSecond[1], specifier: "%.2f") ns/s"); Text("Scheduled \(row.scheduledPerSecond[0], specifier: "%.2f") → \(row.scheduledPerSecond[1], specifier: "%.2f") ns/s"); Text("Polling \(row.pollsPerSecond[0], specifier: "%.2f") → \(row.pollsPerSecond[1], specifier: "%.2f") polls/s"); if let id=row.current {Button("Inspect current entity"){model.select(row.kind,id)}} } } } }
    }.padding() } }
}
struct SharingPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var fields = ""
    @State private var paths = false
    @State private var endpoints = false
    @State private var sections="charts,tasks,resources,comparison"
    private var policy: [String: Any] { ["fields": fields.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }, "source_paths": paths, "endpoints": endpoints] }
    var body: some View { Form {
        Text("Export & share").font(.title); TextField("Redact field keys, comma separated", text: $fields); Toggle("Redact source paths", isOn: $paths); Toggle("Redact endpoint details", isOn: $endpoints)
        Text("Exports use the current filtered snapshot and preserve the source capture.").font(.caption)
        TextField("Included report sections",text:$sections); Button("Apply report sections") {model.send("report_sections",["sections":sections.split(separator:",").map{$0.trimmingCharacters(in:.whitespaces)}])}
        Button("Preview redacted data") {model.send("preview",["policy":policy])}
        if let preview=model.state?.exportPreview { Text("Preview target: \(preview.target ?? "unspecified") · \(preview.snapshot.tasks.count) tasks"); ForEach(Array(preview.snapshot.tasks.prefix(12))) {task in Text("\(task.name) · \(task.location)\n\(task.fields)").font(.caption).textSelection(.enabled)} }

        ForEach(["csv","json","html","svg"], id: \.self) { format in Button("Export \(format.uppercased())…") { model.chooseFile(save: true, extensions: [format]) { path in model.send("export", ["path": path, "format": format, "table": format == "svg" ? "polls" : "tasks", "redaction": policy]) } } }
        Button("Save redacted capture copy…") { model.chooseFile(save: true) { model.send("redact_recording", ["path": $0, "policy": policy]) } }
        if let s = model.state { Text("Export scope: \(s.snapshot.tasks.count) tasks, \(s.snapshot.resources.count) resources, \(s.snapshot.asyncOps.count) async operations") }
    }.formStyle(.grouped) }
}
struct ConnectionsPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var name = ""
    @State private var target = "http://127.0.0.1:6669"
    @State private var rename = ""
    var body: some View { Form {
        Text("Connection profiles").font(.title); TextField("Name", text: $name); TextField("Endpoint", text: $target)
        HStack { Button("Save profile") { model.send("save_profile", ["name": name, "target": target]) }; Button("Open target in new tab") { model.send("add_target", ["name": name, "target": target]) } }
        TextField("New profile name", text: $rename)
        if let state = model.state { ForEach(state.preferences.profiles) { p in HStack { Text("\(p.name) · \(p.target)"); Button("Connect") { model.send("apply_profile", ["name": p.name]) }; Button("Rename") { model.send("rename_profile", ["name": p.name, "new_name": rename]) }; Button("Delete") { model.send("delete_profile", ["name": p.name]) } } }; ForEach(state.preferences.recentTargets, id: \.self) { target in Button(target) { model.send("connect", ["target": target]) } } }
    }.formStyle(.grouped) }
}
struct WorkspacePanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var name = "My investigation"
    @State private var preset = ""
    @State private var originalView = ""
    @State private var editor = "code"
    @State private var arguments = "--goto '{file}:{line}:{column}'"
    @State private var remote = ""
    @State private var local = ""
    var body: some View { Form {
        Picker("Appearance",selection:Binding(get:{model.state?.preferences.settings.palette ?? "system"},set:{model.send("appearance",["palette":$0])})) {Text("System").tag("system");Text("Light").tag("light");Text("Dark").tag("dark")}
        Text("Workspace").font(.title); TextField("Workspace / investigation name", text: $name)
        HStack { Button("New workspace") { model.send("new_workspace", ["name": name]) }; Button("Save workspace…") { model.chooseFile(save: true, extensions: ["json"]) { model.send("save_workspace", ["path": $0, "name": name]) } }; Button("Open workspace…") { model.chooseFile(extensions: ["json"]) { model.send("open_workspace", ["path": $0]) } } }
        Button("Save current investigation view") { model.send("save_view", ["name": name]) }
        TextField("Existing view name",text:$originalView);HStack {Button("Rename view"){model.send("rename_view",["name":originalView,"new_name":name])};Button("Duplicate view"){model.send("duplicate_view",["name":originalView,"new_name":name])}}
        if let s = model.state { ForEach(s.views) { view in HStack { Text(view.name); Button("Restore") { model.send("load_view", ["name": view.name]) }; Button("Delete") { model.send("delete_view", ["name": view.name]) } } } }
        TextField("Filter preset name", text: $preset); Button("Save global filter preset") { model.send("save_preset", ["name": preset]) }; Button("Save project filter preset") {model.send("save_project_preset",["name":preset])}
        if let s = model.state { ForEach(s.preferences.filterPresets) { p in HStack { Text("\(p.name): \(p.expression)"); Button("Apply") { model.send("apply_preset", ["name": p.name]) }; Button("Rename to entered preset name") {model.send("rename_preset",["name":p.name,"new_name":preset,"project":s.projectPresets.contains(p.name)])}; Button("Delete") { model.send(s.projectPresets.contains(p.name) ? "delete_project_preset" : "delete_preset", ["name": p.name]) } } } }
        HStack { Button("Import project policy…") { model.chooseFile(extensions: ["json"]) { model.send("load_policy", ["path": $0]) } }; Button("Export project policy…") { model.chooseFile(save: true, extensions: ["json"]) { model.send("save_policy", ["path": $0]) } } }
        Text("Source editor").font(.headline); TextField("Executable", text: $editor); TextField("Arguments", text: $arguments); TextField("Remote source root", text: $remote); TextField("Local source root", text: $local)
        Button("Save source-editor integration") { var mappings: [[String: String]] = []; if !remote.isEmpty { mappings.append(["remote_root": remote,"local_root": local]) }; model.send("editor_command", ["program": editor,"arguments": arguments,"mappings": mappings]) }
        Text("Policy precedence: application defaults → project → explicit session overrides.").font(.caption)
    }.formStyle(.grouped).onAppear {if let e=model.state?.preferences.editor {editor=e.program; arguments=e.arguments.map { "\""+$0.replacingOccurrences(of:"\"",with:"\\\"")+"\"" }.joined(separator:" "); if let mapping=e.mappings.first {remote=mapping.remoteRoot;local=mapping.localRoot}}} }
}
struct LookupPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var query = ""
    @FocusState private var focused: Bool
    var body: some View { VStack(alignment: .leading) { Text("Find a task").font(.title2); TextField("Task name or ID", text: $query).focused($focused); List(model.state?.lookupTasks ?? []) { task in Button("\(task.id) · \(task.name) · \(task.location)") { model.select("task",task.id); model.lookup = false } }; Button("Close") { model.lookup = false }.keyboardShortcut(.cancelAction) }.padding().frame(width: 700,height: 480).onAppear { focused = true;model.send("lookup",["text":query]) }.onChange(of:query){_,value in model.send("lookup",["text":value])} }
}
struct ChartPoint: Identifiable {let now:UInt64;let value:Double;let segment:Int;var id:UInt64 {now}}
struct TrendChart: View {
    let points:[Trend];let metric:String
    var samples:[ChartPoint] {var segment=0;return points.compactMap {p in if p.gap {segment+=1;return nil};return ChartPoint(now:p.now,value:metric == "busy" ? Double(p.busyNs) : metric == "scheduled" ? Double(p.scheduledNs) : p.pollsPerSecond,segment:segment)}}
    var body:some View {VStack(alignment:.leading){Text("\(metric) · \(metric == "polls" ? "polls/s" : "ns/interval")").font(.headline);Chart(samples){p in LineMark(x:.value("Producer time (ns)",p.now),y:.value("Value",p.value),series:.value("Contiguous interval",p.segment)).foregroundStyle(.blue)}.frame(height:180);Text("Gaps split the line. Producer time is in nanoseconds.").font(.caption)}}
}
func smokeScreenshot() throws {
    guard let view=NSApplication.shared.windows.first(where:{$0.isVisible})?.contentView, let bitmap=view.bitmapImageRepForCachingDisplay(in:view.bounds) else {throw NSError(domain:"ConsoleUI",code:1,userInfo:[NSLocalizedDescriptionKey:"Native window is unavailable"])}
    view.cacheDisplay(in:view.bounds,to:bitmap)
    guard let png=bitmap.representation(using:.png,properties:[:]) else {throw NSError(domain:"ConsoleUI",code:2)}
    try FileManager.default.createDirectory(atPath:"dist",withIntermediateDirectories:true)
    try png.write(to:URL(fileURLWithPath:FileManager.default.currentDirectoryPath).appendingPathComponent("dist/smoke-macos.png"))
}
func metric(_ name: String, _ value: String) -> some View { HStack { Text(name).foregroundStyle(.secondary); Spacer(); Text(value).monospacedDigit() } }
func ns(_ value: UInt64) -> String { if value < 1000 { return "\(value) ns" }; if value < 1_000_000 { return String(format: "%.2f µs",Double(value)/1e3) }; if value < 1_000_000_000 { return String(format: "%.2f ms",Double(value)/1e6) }; return String(format: "%.2f s",Double(value)/1e9) }
func percent(_ value: UInt64, _ total: UInt64) -> String { String(format:"%.1f", total == 0 ? 0 : Double(value)/Double(total)*100) }
func durationNs(_ value: String) -> UInt64? { for (suffix, factor) in [("ms",1e6),("us",1e3),("ns",1.0),("s",1e9),("min",60e9)] { if value.hasSuffix(suffix), let number = Double(value.dropLast(suffix.count)), number.isFinite, number > 0, number*factor < Double(UInt64.max) { return UInt64(number*factor) } }; return nil }
func splitArguments(_ value: String) -> [String]? { var args:[String] = []; var current = ""; var quote: Character?; for c in value { if c == "'" || c == "\"" { if quote == c { quote = nil } else if quote == nil { quote = c } else { current.append(c) } } else if c.isWhitespace && quote == nil { if !current.isEmpty { args.append(current); current = "" } } else { current.append(c) } }; if quote != nil {return nil}; if !current.isEmpty { args.append(current) }; return args }

@MainActor final class AppDelegate: NSObject, NSApplicationDelegate { weak var model:RuntimeModel?; func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {true};func applicationWillTerminate(_ notification:Notification) {model?.shutdown()} }
@main struct ConsoleUIApplication: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var model = RuntimeModel()
    var body: some Scene { WindowGroup("Console UI") { ContentView(model: model).frame(minWidth: 1100,minHeight: 700).onAppear {delegate.model=model} }.defaultSize(width: 1540,height: 960).commands {
        SidebarCommands()
        CommandMenu("Diagnostics") { Button("Find task…") { model.lookup = true }.keyboardShortcut("k",modifiers:.command); Button("Pause / resume target") { model.send("pause") }.keyboardShortcut("p",modifiers:[.command,.shift]); Button("Open recording…") { model.chooseFile { model.send("open_recording",["path":$0,"recover":false]) } }.keyboardShortcut("o",modifiers:.command); Button("Open selected source") { model.send("open_source") }.keyboardShortcut("j",modifiers:[.command,.shift]) }
    } }
}
