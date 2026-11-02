"""
PlatformIO pre-build script: build the Rust chess core (chess/) and link it.

Defines CROSSCHESS_ENABLED=1 for MCUs that have a Rust target below. Other
boards build without chess. Also defines CROSSCHESS_BOOT_GUARD for the boards
in BOOT_GUARD_MCUS (src/crosschess/BootGuard.h); that list is separate because
the guard's Side Down shortcut clashes with Recovery mode on some S3 boards.
"""

import os
import shutil
import subprocess
import sys

Import("env")  # noqa: F821 - provided by PlatformIO

RUST_TARGETS = {
    "esp32c3": "riscv32imc-unknown-none-elf",
}
BOOT_GUARD_MCUS = {"esp32c3"}
# Toolchain whose objcopy prepares the library, per Rust target.
TOOLCHAINS = {
    "riscv32imc-unknown-none-elf": ("toolchain-riscv32-esp", "riscv32-esp-elf-objcopy"),
}


def find_cargo():
    cargo = shutil.which("cargo")
    if cargo:
        return cargo
    exe = "cargo.exe" if os.name == "nt" else "cargo"
    fallback = os.path.join(os.path.expanduser("~"), ".cargo", "bin", exe)
    return fallback if os.path.isfile(fallback) else None


def localize_library(source, destination, target):
    """Keeps only the cc_* C API global in the Rust library.

    The library carries Rust's compiler_builtins, which also defines C maths
    and libgcc helpers (roundf, pow, sqrt, ...). Linked before libm and libgcc,
    those would quietly replace the ones the rest of the firmware uses.
    """
    package, tool = TOOLCHAINS[target]
    toolchain = env.PioPlatform().get_package_dir(package)  # noqa: F821
    objcopy = os.path.join(toolchain, "bin", tool + (".exe" if os.name == "nt" else ""))
    os.makedirs(os.path.dirname(destination), exist_ok=True)
    result = subprocess.run([objcopy, "--wildcard", "--keep-global-symbol=cc_*", source, destination])
    if result.returncode != 0:
        sys.exit("build_chess_core.py: objcopy failed")


mcu = env.BoardConfig().get("build.mcu", "")  # noqa: F821
target = RUST_TARGETS.get(mcu)

if mcu in BOOT_GUARD_MCUS:
    env.Append(CPPDEFINES=[("CROSSCHESS_BOOT_GUARD", 1)])  # noqa: F821

if target is None:
    print(f"build_chess_core.py: no Rust target for {mcu}, building without chess")
else:
    chess_dir = os.path.join(env.subst("$PROJECT_DIR"), "chess")  # noqa: F821
    cargo = find_cargo()
    if cargo is None:
        sys.exit(f"build_chess_core.py: cargo not found. Install Rust and run: rustup target add {target}")

    result = subprocess.run(
        [cargo, "build", "--release", "--locked", "-p", "crosschess-ffi", "--target", target],
        cwd=chess_dir,
    )
    if result.returncode != 0:
        sys.exit("build_chess_core.py: cargo build failed")

    release_dir = os.path.join(chess_dir, "target", target, "release")
    firmware_dir = os.path.join(release_dir, "firmware")
    localize_library(
        os.path.join(release_dir, "libcrosschess_ffi.a"),
        os.path.join(firmware_dir, "libcrosschess_ffi.a"),
        target,
    )

    env.Append(  # noqa: F821
        CPPDEFINES=[("CROSSCHESS_ENABLED", 1)],
        CPPPATH=[os.path.join(chess_dir, "include")],
        LIBPATH=[firmware_dir],
        LIBS=["crosschess_ffi"],
    )
