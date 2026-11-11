# Third-party components

Dependency versions and source revisions are recorded in Cargo.lock. This project
uses windows-rs/windows-reactor (MIT OR Apache-2.0), Tokio and console-api
(MIT), tonic/prost (MIT), hdrhistogram (MIT OR Apache-2.0), serde (MIT OR Apache-2.0),
clap (MIT OR Apache-2.0), and their transitive dependencies. Refer to each crate's
published license files for its complete terms and notices.

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
