# Third-party components

Dependency versions and source revisions are recorded in Cargo.lock. This project
uses windows-rs/windows-reactor (MIT OR Apache-2.0), Tokio and console-api
(MIT), tonic/prost (MIT), hdrhistogram (MIT OR Apache-2.0), serde (MIT OR Apache-2.0),
clap (MIT OR Apache-2.0), and their transitive dependencies. Refer to each crate's
published license files for its complete terms and notices.

The Linux frontend uses Relm4 (Apache-2.0 OR MIT), gtk-rs (MIT), and the system GTK4,
GLib/GIO, Cairo, Pango, and GDK-Pixbuf libraries under their respective LGPL terms.
The Linux archive dynamically links these system libraries and does not bundle them.
Sources and license details: https://relm4.org/ and https://www.gtk.org/.
The macOS frontend uses system SwiftUI, AppKit, and Charts frameworks; it bundles
this project's Rust library rather than redistributing Apple frameworks.

The self-contained Windows distribution includes Microsoft's Windows App Runtime,
WebView2 projection assembly, and Visual C++ runtime redistributables under their
respective Microsoft redistribution terms. The runtime payload and versions are
selected by the pinned windows-reactor-setup helper. Runtime source and package
license information:

* https://github.com/microsoft/WindowsAppSDK
* https://www.nuget.org/packages/Microsoft.WindowsAppSDK.Runtime
* https://www.nuget.org/packages/Microsoft.Web.WebView2
* https://visualstudio.microsoft.com/license-terms/

This application is an independent diagnostic client and is not an official
Tokio or Microsoft product.
