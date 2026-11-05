# 1. Create the staging folders and copy the scalar (x86-64-v2) GPU build that
#    scripts/pgo-bolt-build.sh writes to bin\, under the name the manifest launches
New-Item -ItemType Directory -Force -Path .\msix_build\assets | Out-Null
Copy-Item .\bin\pleochroma-cut-windows-scalar-gpu.exe .\msix_build\pleochroma-cut.exe

# 2. Copy the package manifest and the logos it references (msix\AppxManifest.xml is the
#    source; bump its Version there for every Store submission)
Copy-Item .\msix\AppxManifest.xml .\msix_build\
Copy-Item .\msix\assets\StoreLogo.png, .\msix\assets\Square44x44Logo.png, .\msix\assets\Square150x150Logo.png .\msix_build\assets\

# 3. Pack the MSIX using your 10.0.22621.0 SDK
& "C:\Program Files (x86)\Windows Kits\10\bin\10.0.22621.0\x64\makeappx.exe" pack /d .\msix_build /p .\pleochroma-cut.msix /o
