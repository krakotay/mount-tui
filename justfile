set shell := ["bash", "-euo", "pipefail", "-c"]

binary := "mount-tui"
prefix := env_var_or_default("PREFIX", "/usr/local")
dist := "dist"

# Show available recipes.
default:
    @just --list

# Build an optimized binary.
build:
    cargo build --release

# Build and install the binary into PREFIX/bin (default: /usr/local/bin).
install: build
    sudo install -Dm0755 "target/release/{{ binary }}" "{{ prefix }}/bin/{{ binary }}"

# Build a Debian package at dist/mount-tui.deb.
build-deb: build
    command -v cargo-deb >/dev/null || cargo install cargo-deb --version 3.7.0 --locked
    mkdir -p "{{ dist }}"
    cargo deb --no-build --output "{{ dist }}/{{ binary }}.deb"

# Build and install the Debian package, including dependencies when apt is available.
install-deb: build-deb
    if command -v apt >/dev/null; then sudo apt install -y "./{{ dist }}/{{ binary }}.deb"; elif command -v dpkg >/dev/null; then sudo dpkg -i "{{ dist }}/{{ binary }}.deb"; else echo "Neither apt nor dpkg is available" >&2; exit 1; fi

# Build an RPM package at dist/mount-tui.rpm.
build-rpm: build
    command -v cargo-generate-rpm >/dev/null || cargo install cargo-generate-rpm --version 0.21.0 --locked
    mkdir -p "{{ dist }}"
    cargo generate-rpm --output "{{ dist }}/{{ binary }}.rpm"

# Build and install the RPM with the system package manager.
install-rpm: build-rpm
    if command -v dnf >/dev/null; then sudo dnf install -y "./{{ dist }}/{{ binary }}.rpm"; elif command -v zypper >/dev/null; then sudo zypper --non-interactive install --allow-unsigned-rpm "./{{ dist }}/{{ binary }}.rpm"; elif command -v rpm >/dev/null; then sudo rpm -Uvh --replacepkgs "{{ dist }}/{{ binary }}.rpm"; else echo "No RPM package manager is available" >&2; exit 1; fi

# Run formatting, tests, and lints.
check:
    cargo fmt --check
    cargo test
    cargo clippy --all-targets --all-features -- -D warnings
