import SwiftUI
import AppKit
import Charts
import Darwin

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
struct Preferences: Decodable { let profiles: [Profile]; let recentTargets: [String]; let filterPresets: [Preset] }
struct Trend: Decodable, Identifiable { let now, busyNs, scheduledNs: UInt64; let pollsPerSecond: Double; let gap: Bool; var id: UInt64 { now } }
struct Evidence: Decodable, Identifiable { let task: UInt64; let lint, measured, threshold, explanation, source: String; var id: String { "\(task):\(lint)" } }
struct GraphNode: Decodable, Identifiable { let key, label, kind: String; var id: String { key } }
struct GraphEdge: Decodable, Identifiable { let from, to, label: String; let operation: UInt64?; var id: String { "\(from):\(to):\(operation ?? 0)" } }
struct Graph: Decodable { let nodes: [GraphNode]; let edges: [GraphEdge]; let truncated: Bool }
struct ComparisonRow: Decodable, Identifiable { let key, name, status: String; let baseline, current: UInt64?; let busyPerSecond, scheduledPerSecond, pollsPerSecond: [Double]; let regression: Bool; var id: String { key } }
struct Comparison: Decodable { let rows: [ComparisonRow]; let notes: [String] }
struct RuntimeView: Decodable { let name: String; let active: UInt64; let tabs: [TargetTab]; let status: String; let paused: Bool; let target: String; let recording: String?; let session: UInt64; let snapshot: Snapshot; let query: String; let selection: Selection?; let trends: [Trend]; let evidence: [Evidence]; let graph: Graph; let sessions: [SessionInfo]; let cursor: UInt64?; let playing: Bool; let speed: Double; let bookmarks: [Bookmark]; let views: [SavedView]; let preferences: Preferences; let comparison: Comparison?; let notice: String?; let pending: Int }

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
        case 3 where !state.paused: print("NATIVE_MACOS_SMOKE_OK"); shutdown(); NSApplication.shared.terminate(nil)
        default: break
        }
        if Date().timeIntervalSince(started) > 40 { fputs("Native macOS smoke timed out\n", stderr); shutdown(); exit(1) }
    }
    func chooseFile(save: Bool = false, extensions: [String] = ["tcui"], action: @escaping (String) -> Void) {
        if save { let panel = NSSavePanel(); panel.canCreateDirectories = true; panel.nameFieldStringValue = "capture.\(extensions[0])"; panel.begin { response in if response == .OK, let url = panel.url { action(url.path) } } }
        else { let panel = NSOpenPanel(); panel.canChooseDirectories = false; panel.allowsMultipleSelection = false; panel.begin { response in if response == .OK, let url = panel.url { action(url.path) } } }
    }
    func select(_ kind: String, _ id: UInt64) { send("select", ["kind": kind, "id": id]) }
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
                    ScrollView(.horizontal) { HStack { ForEach(state.tabs) { tab in HStack { Button(tab.name + (tab.recording == nil ? "" : " ● REC")) { model.send("select_tab", ["id": tab.id]) }.buttonStyle(.bordered); if state.tabs.count > 1 { Button { model.send("close_tab", ["id": tab.id]) } label: { Image(systemName: "xmark") }.help("Close \(tab.name)") } } } }
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
        .onChange(of: model.state?.active) { _, _ in endpoint = model.state?.target ?? endpoint; query = model.state?.query ?? "" }
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
            if selected.kind == "task", let t = state.snapshot.tasks.first(where: { $0.id == selected.id }) {
                Text("\(t.name) / task \(t.id)").font(.title2)
                Text("\(t.state) · \(t.blocking ? "blocking" : "async")")
                ForEach(t.warnings, id: \.self) { Text("Warning: \($0)").foregroundStyle(.orange) }
                GroupBox("Task timing") { VStack(alignment: .leading) { metric("Total", ns(t.timing.total)); metric("Busy", "\(ns(t.timing.busy)) (\(percent(t.timing.busy,t.timing.total))%)"); metric("Scheduled", "\(ns(t.timing.scheduled)) (\(percent(t.timing.scheduled,t.timing.total))%)"); metric("Idle", ns(t.timing.idle)); metric("Polls", String(t.timing.polls)) } }
                GroupBox("Waker activity") { VStack(alignment: .leading) { metric("Active", String(t.wakerClones >= t.wakerDrops ? t.wakerClones-t.wakerDrops : 0)); metric("Clones / drops", "\(t.wakerClones) / \(t.wakerDrops)"); metric("Self wakes", "\(t.selfWakes) (\(percent(t.selfWakes,t.wakes))%)"); metric("Last woken", t.lastWakeAgo.map { ns($0) + " ago" } ?? "Never") } }
                if let details = state.snapshot.details[String(t.id)] { HistogramPanel(name: "Poll times", histogram: details.poll); HistogramPanel(name: "Scheduled times", histogram: details.scheduled) } else { ProgressView("Waiting for task details…") }
                Text(t.location).textSelection(.enabled); Text(t.fields).font(.system(.caption, design: .monospaced)).textSelection(.enabled); Button("Open source in editor") { model.send("open_source") }
            } else if selected.kind == "resource", let r = state.snapshot.resources.first(where: { $0.id == selected.id }) {
                Text("\(r.concreteType) / resource \(r.id)").font(.title2); Text("\(r.kind) · \(r.internal ? "INT" : "PUB") · \(ns(r.total))"); Text(r.location).textSelection(.enabled); Text(r.attributes).textSelection(.enabled)
                ForEach(state.snapshot.asyncOps.filter { $0.resourceId == r.id }) { op in GroupBox(op.source) { VStack(alignment: .leading) { Text("Task \(op.taskId ?? 0) \(op.taskName)"); Text("\(op.timing.polls) polls · \(ns(op.timing.busy)) busy · \(ns(op.timing.idle)) idle"); Text(op.attributes); if let id = op.taskId { Button("Inspect waiting task") { model.select("task", id) } } } } }
            } else if let op = state.snapshot.asyncOps.first(where: { $0.id == selected.id }) { Text(op.source).font(.title2); Text(op.attributes); if let id = op.taskId { Button("Inspect operation's task") { model.select("task", id) } } }
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
    @State private var wake = "50", never = "1s", future = "1024"
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        Text("Runtime analysis").font(.title)
        HStack { TextField("Window seconds", text: $seconds); Button("Apply window") { if let s = UInt64(seconds), s > 0 { model.send("window", ["nanoseconds": s*1_000_000_000]) } else { model.error = "Enter a positive number of seconds" } } }
        if let state = model.state {
            Chart(state.trends.filter { !$0.gap }) { point in LineMark(x: .value("Producer timestamp (ns)", point.now), y: .value("Polls/s", point.pollsPerSecond)) }.frame(height: 220).chartYAxisLabel("Polls / second")
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
    @State private var selectedSession = 0
    @State private var offset = 0.0
    @State private var dragging = false
    @State private var bookmarkName = "", note = ""
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        Text("Recording and replay").font(.title)
        HStack { Button("Start recording…") { model.chooseFile(save: true) { model.send("start_recording", ["path": $0]) } }; Button("Stop recording") { model.send("stop_recording") }; Button("Open recording…") { model.chooseFile { model.send("open_recording", ["path": $0, "recover": false]) } }; Button("Recover partial capture…") { model.chooseFile { model.send("open_recording", ["path": $0, "recover": true]) } } }
        if let state = model.state, !state.sessions.isEmpty {
            Picker("Session", selection: $selectedSession) { ForEach(state.sessions) { session in Text("Session \(session.index): \(session.start)–\(session.end) ns\(session.incomplete ? " (incomplete)" : "")").tag(session.index) } }.onChange(of: selectedSession) { _, index in model.send("session", ["index": index]) }
            if let session = state.sessions.first(where: { $0.index == selectedSession }) {
                Slider(value: $offset, in: 0...max(0.001,Double(session.end-session.start)/1e9), onEditingChanged: { editing in dragging = editing; if !editing { model.send("seek", ["cursor": session.start + UInt64(offset*1e9)]) } })
                Text("Cursor \(state.cursor ?? session.start) ns · offset \(offset, specifier: "%.3f")s")
                HStack { Button(state.playing ? "Pause playback" : "Play recording") { model.send("playback") }; ForEach([0.5,1.0,2.0,4.0], id: \.self) { speed in Button("\(speed, specifier: "%.1f")×") { model.send("speed", ["speed": speed]) } } }
                .onChange(of: state.cursor) { _, cursor in if !dragging, let cursor, cursor >= session.start { offset = Double(cursor-session.start)/1e9 } }
            }
            HStack { TextField("Bookmark name", text: $bookmarkName); TextField("Investigation note", text: $note); Button("Bookmark cursor") { model.send("bookmark", ["name": bookmarkName, "note": note]) } }
            ForEach(state.bookmarks) { bookmark in HStack { Text("\(bookmark.name) · \(bookmark.cursor) ns · \(bookmark.note)"); Button("Go") { model.send("go_bookmark", ["id": bookmark.id]); selectedSession = bookmark.session }; Button("Delete") { model.send("delete_bookmark", ["id": bookmark.id]) } } }
        }
    }.padding() } }
}
struct ComparePanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var session = "0", relative = "10", absoluteMs = "1", polls = "100"
    var body: some View { ScrollView { VStack(alignment: .leading, spacing: 16) {
        Text("Compare captures").font(.title); Text("Open the current capture, then select a baseline. Values use interval-normalized timing and polling.")
        HStack { TextField("Baseline session", text: $session); TextField("Relative %", text: $relative); TextField("Absolute ms", text: $absoluteMs); TextField("Polls/s increase", text: $polls) }
        Button("Choose baseline…") { if let index = Int(session), index >= 0, let percent = Double(relative), percent.isFinite, percent >= 0, let ms = Double(absoluteMs), ms.isFinite, ms >= 0, let polling = UInt64(polls) { model.chooseFile { model.send("compare", ["path": $0, "session": index, "policy": ["relative_percent": percent, "absolute_ns": UInt64(ms*1e6), "polls_increase": polling]]) } } else { model.error = "Enter valid session and regression thresholds" } }
        if let comparison = model.state?.comparison { ForEach(comparison.notes, id: \.self) { Text($0).font(.caption) }; ForEach(comparison.rows) { row in GroupBox("\(row.name) · \(row.status)") { VStack(alignment: .leading) { Text("Busy \(row.busyPerSecond[0], specifier: "%.2f") → \(row.busyPerSecond[1], specifier: "%.2f") ns/s"); Text("Scheduled \(row.scheduledPerSecond[0], specifier: "%.2f") → \(row.scheduledPerSecond[1], specifier: "%.2f") ns/s"); Text("Polling \(row.pollsPerSecond[0], specifier: "%.2f") → \(row.pollsPerSecond[1], specifier: "%.2f") polls/s") } } } }
    }.padding() } }
}
struct SharingPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var fields = "", paths = false, endpoints = false
    private var policy: [String: Any] { ["fields": fields.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }, "source_paths": paths, "endpoints": endpoints] }
    var body: some View { Form {
        Text("Export & share").font(.title); TextField("Redact field keys, comma separated", text: $fields); Toggle("Redact source paths", isOn: $paths); Toggle("Redact endpoint details", isOn: $endpoints)
        Text("Exports use the current filtered snapshot and preserve the source capture.").font(.caption)
        ForEach(["csv","json","html","svg"], id: \.self) { format in Button("Export \(format.uppercased())…") { model.chooseFile(save: true, extensions: [format]) { path in model.send("export", ["path": path, "format": format, "table": format == "svg" ? "polls" : "tasks", "redaction": policy]) } } }
        Button("Save redacted capture copy…") { model.chooseFile(save: true) { model.send("redact_recording", ["path": $0, "policy": policy]) } }
        if let s = model.state { Text("Export scope: \(s.snapshot.tasks.count) tasks, \(s.snapshot.resources.count) resources, \(s.snapshot.asyncOps.count) async operations") }
    }.formStyle(.grouped) }
}
struct ConnectionsPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var name = "", target = "http://127.0.0.1:6669", rename = ""
    var body: some View { Form {
        Text("Connection profiles").font(.title); TextField("Name", text: $name); TextField("Endpoint", text: $target)
        HStack { Button("Save profile") { model.send("save_profile", ["name": name, "target": target]) }; Button("Open target in new tab") { model.send("add_target", ["name": name, "target": target]) } }
        TextField("New profile name", text: $rename)
        if let state = model.state { ForEach(state.preferences.profiles) { p in HStack { Text("\(p.name) · \(p.target)"); Button("Connect") { model.send("apply_profile", ["name": p.name]) }; Button("Rename") { model.send("rename_profile", ["name": p.name, "new_name": rename]) }; Button("Delete") { model.send("delete_profile", ["name": p.name]) } } }; ForEach(state.preferences.recentTargets, id: \.self) { target in Button(target) { model.send("connect", ["target": target]) } } }
    }.formStyle(.grouped) }
}
struct WorkspacePanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var name = "My investigation", preset = "", editor = "code", arguments = "--goto '{file}:{line}:{column}'", remote = "", local = ""
    var body: some View { Form {
        Text("Workspace").font(.title); TextField("Workspace / investigation name", text: $name)
        HStack { Button("New workspace") { model.send("new_workspace", ["name": name]) }; Button("Save workspace…") { model.chooseFile(save: true, extensions: ["json"]) { model.send("save_workspace", ["path": $0, "name": name]) } }; Button("Open workspace…") { model.chooseFile(extensions: ["json"]) { model.send("open_workspace", ["path": $0]) } } }
        Button("Save current investigation view") { model.send("save_view", ["name": name]) }
        if let s = model.state { ForEach(s.views) { view in HStack { Text(view.name); Button("Restore") { model.send("load_view", ["name": view.name]) }; Button("Delete") { model.send("delete_view", ["name": view.name]) } } } }
        TextField("Filter preset name", text: $preset); Button("Save current filter preset") { model.send("save_preset", ["name": preset]) }
        if let s = model.state { ForEach(s.preferences.filterPresets) { p in HStack { Text("\(p.name): \(p.expression)"); Button("Apply") { model.send("apply_preset", ["name": p.name]) }; Button("Delete") { model.send("delete_preset", ["name": p.name]) } } } }
        HStack { Button("Import project policy…") { model.chooseFile(extensions: ["json"]) { model.send("load_policy", ["path": $0]) } }; Button("Export project policy…") { model.chooseFile(save: true, extensions: ["json"]) { model.send("save_policy", ["path": $0]) } } }
        Text("Source editor").font(.headline); TextField("Executable", text: $editor); TextField("Arguments", text: $arguments); TextField("Remote source root", text: $remote); TextField("Local source root", text: $local)
        Button("Save source-editor integration") { let args = splitArguments(arguments); var mappings: [[String: String]] = []; if !remote.isEmpty { mappings.append(["remote_root": remote,"local_root": local]) }; model.send("editor", ["preferences": ["program": editor,"arguments": args,"mappings": mappings]]) }
        Text("Policy precedence: application defaults → project → explicit session overrides.").font(.caption)
    }.formStyle(.grouped) }
}
struct LookupPanel: View {
    @ObservedObject var model: RuntimeModel
    @State private var query = ""
    @FocusState private var focused: Bool
    var body: some View { VStack(alignment: .leading) { Text("Find a task").font(.title2); TextField("Task name or ID", text: $query).focused($focused); List((model.state?.snapshot.tasks ?? []).filter { query.isEmpty || $0.name.localizedCaseInsensitiveContains(query) || String($0.id) == query }) { task in Button("\(task.id) · \(task.name) · \(task.location)") { model.select("task",task.id); model.lookup = false } }; Button("Close") { model.lookup = false }.keyboardShortcut(.cancelAction) }.padding().frame(width: 700,height: 480).onAppear { focused = true } }
}
func metric(_ name: String, _ value: String) -> some View { HStack { Text(name).foregroundStyle(.secondary); Spacer(); Text(value).monospacedDigit() } }
func ns(_ value: UInt64) -> String { if value < 1000 { return "\(value) ns" }; if value < 1_000_000 { return String(format: "%.2f µs",Double(value)/1e3) }; if value < 1_000_000_000 { return String(format: "%.2f ms",Double(value)/1e6) }; return String(format: "%.2f s",Double(value)/1e9) }
func percent(_ value: UInt64, _ total: UInt64) -> String { String(format:"%.1f", total == 0 ? 0 : Double(value)/Double(total)*100) }
func durationNs(_ value: String) -> UInt64? { for (suffix, factor) in [("ms",1e6),("us",1e3),("ns",1.0),("s",1e9),("min",60e9)] { if value.hasSuffix(suffix), let number = Double(value.dropLast(suffix.count)), number.isFinite, number > 0, number*factor < Double(UInt64.max) { return UInt64(number*factor) } }; return nil }
func splitArguments(_ value: String) -> [String] { var args:[String] = []; var current = ""; var quote: Character?; for c in value { if c == "'" || c == "\"" { if quote == c { quote = nil } else if quote == nil { quote = c } else { current.append(c) } } else if c.isWhitespace && quote == nil { if !current.isEmpty { args.append(current); current = "" } } else { current.append(c) } }; if !current.isEmpty { args.append(current) }; return args }

final class AppDelegate: NSObject, NSApplicationDelegate { func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true } }
@main struct ConsoleUIApplication: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var model = RuntimeModel()
    var body: some Scene { WindowGroup("Console UI") { ContentView(model: model).frame(minWidth: 1100,minHeight: 700) }.defaultSize(width: 1540,height: 960).commands {
        SidebarCommands()
        CommandMenu("Diagnostics") { Button("Find task…") { model.lookup = true }.keyboardShortcut("k",modifiers:.command); Button("Pause / resume target") { model.send("pause") }.keyboardShortcut("p",modifiers:[.command,.shift]); Button("Open recording…") { model.chooseFile { model.send("open_recording",["path":$0,"recover":false]) } }.keyboardShortcut("o",modifiers:.command); Button("Open selected source") { model.send("open_source") }.keyboardShortcut("j",modifiers:[.command,.shift]) }
    } }
}
