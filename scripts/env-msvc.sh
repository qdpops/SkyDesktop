# Окружение для сборки RustDesk с MSVC Build Tools (per-user) + Windows SDK.
# Использование: source scripts/env-msvc.sh

export MSVC_VER="14.44.35207"
export SDK_VER="10.0.26100.0"
export VSBT="/c/Users/dev/vsbt2022"
export SDK="/c/Program Files (x86)/Windows Kits/10"

# MSVC (cl/link/nmake) и Windows SDK (rc.exe) — первыми в PATH,
# чтобы не подхватился link.exe из MSYS.
export PATH="$VSBT/VC/Tools/MSVC/$MSVC_VER/bin/Hostx64/x64:$SDK/bin/$SDK_VER/x64:$USERPROFILE/.cargo/bin:/c/Users/dev/AppData/Local/Packages/PythonSoftwareFoundation.Python.3.13_qbz5n2kfra8p0/LocalCache/local-packages/Python313/Scripts:$PATH"

export LIB="C:/Users/dev/vsbt2022/VC/Tools/MSVC/$MSVC_VER/lib/x64;C:/Program Files (x86)/Windows Kits/10/Lib/$SDK_VER/um/x64;C:/Program Files (x86)/Windows Kits/10/Lib/$SDK_VER/ucrt/x64"
export INCLUDE="C:/Users/dev/vsbt2022/VC/Tools/MSVC/$MSVC_VER/include;C:/Program Files (x86)/Windows Kits/10/Include/$SDK_VER/ucrt;C:/Program Files (x86)/Windows Kits/10/Include/$SDK_VER/um;C:/Program Files (x86)/Windows Kits/10/Include/$SDK_VER/shared"

# vcpkg и VS для него.
# ВАЖНО: для cargo/build.rs пути должны быть Windows-стиля (Rust PathBuf),
# а не MSYS-стиля /c/... — иначе не найдутся installed/<triplet>/lib.
SKYDESKTOP_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export VCPKG_ROOT="${VCPKG_ROOT:-$(cygpath -m "$SKYDESKTOP_ROOT/vcpkg")}"
export VCPKG_INSTALLED_ROOT="${VCPKG_INSTALLED_ROOT:-$VCPKG_ROOT/installed}"
export VCPKG_VISUAL_STUDIO_PATH="C:\\Users\\dev\\vsbt2022"

# magnum-opus (Opus) собирается через cmake; VS-генератор недоступен без vswhere,
# поэтому явно используем NMake.
export CMAKE_GENERATOR="NMake Makefiles"

# bindgen (scrap) требует libclang.dll — берём из PyPI-колеса libclang.
export LIBCLANG_PATH="C:/Users/dev/AppData/Local/Packages/PythonSoftwareFoundation.Python.3.13_qbz5n2kfra8p0/LocalCache/local-packages/Python313/site-packages/clang/native"
