# Microsoft Visual C++ Runtime deployment notice

UTranslate uses app-local deployment for the retail Visual C++ runtime DLLs required by the official ONNX Runtime Windows binary. The files are copied from the `VC/Redist/MSVC/.../x64/Microsoft.VC143.CRT` REDIST directory of the licensed Visual Studio Build Tools installation used to build the app.

Microsoft documents application-local deployment and the permitted REDIST list here:

- https://learn.microsoft.com/cpp/windows/deployment-in-visual-cpp
- https://learn.microsoft.com/cpp/windows/determining-which-dlls-to-redistribute
- https://visualstudio.microsoft.com/license-terms/

The generated OCR manifest records the exact file version, SHA-256, size, signer, REDIST source kind, and toolset version for every copied DLL. Universal CRT is a Windows 10/11 system component and is not bundled.
