
echo "🔨 QCLang Quantum Compiler Build Script"
echo "======================================"

# Configuration
VERSION="0.4.1"
PROJECT_ROOT="/mnt/c/CODE/qclang"
COMPILER_DIR="$PROJECT_ROOT/compiler"
DIST_DIR="$PROJECT_ROOT/dist"
SCRIPTS_DIR="$PROJECT_ROOT/script"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}🚀 Starting build process...${NC}"

# Check if we're in the right directory
if [ ! -f "$COMPILER_DIR/Cargo.toml" ]; then
    echo -e "${RED}❌ Error: Cargo.toml not found in $COMPILER_DIR${NC}"
    echo "Make sure you're running this from the qclang project root"
    exit 1
fi

cd "$COMPILER_DIR"

# Clean up previous builds
echo -e "${YELLOW}🧹 Cleaning previous builds...${NC}"
cargo clean 2>/dev/null || true
rm -rf "$DIST_DIR" 2>/dev/null || true
mkdir -p "$DIST_DIR"

# 1. Build Linux binary (statically linked)
echo -e "${YELLOW}🐧 Building Linux binary (x86_64, static)...${NC}"
if ! rustup target list | grep -q "x86_64-unknown-linux-gnu (installed)"; then
    echo "Installing Linux target..."
    rustup target add x86_64-unknown-linux-gnu
fi

RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✅ Linux build successful${NC}"
else
    echo -e "${RED}❌ Linux build failed${NC}"
    exit 1
fi

# 2. Build Windows binary (using MSVC for best compatibility)
echo -e "${YELLOW}🪟 Building Windows binary (x86_64, MSVC)...${NC}"
if ! rustup target list | grep -q "x86_64-pc-windows-msvc (installed)"; then
    echo "Installing Windows MSVC target..."
    rustup target add x86_64-pc-windows-msvc
fi

# Build for Windows (MSVC)
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS='-C target-feature=+crt-static' \
    cargo build --release --target x86_64-pc-windows-msvc

if [ $? -eq 0 ]; then
    echo -e "${GREEN}✅ Windows build successful${NC}"
else
    echo -e "${RED}❌ Windows build failed${NC}"
    echo "Trying GNU target as fallback..."
    # Fallback to GNU target
    rustup target add x86_64-pc-windows-gnu
    RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-pc-windows-gnu
fi

# 3. Prepare distribution
echo -e "${YELLOW}📁 Preparing distribution...${NC}"
mkdir -p "$DIST_DIR/linux"
mkdir -p "$DIST_DIR/windows"

# Copy Linux binary
cp target/x86_64-unknown-linux-gnu/release/qclang "$DIST_DIR/linux/qclang"
strip "$DIST_DIR/linux/qclang" 2>/dev/null || true

# Copy Windows binary (try MSVC first, then GNU)
if [ -f "target/x86_64-pc-windows-msvc/release/qclang.exe" ]; then
    cp target/x86_64-pc-windows-msvc/release/qclang.exe "$DIST_DIR/windows/qclang.exe"
elif [ -f "target/x86_64-pc-windows-gnu/release/qclang.exe" ]; then
    cp target/x86_64-pc-windows-gnu/release/qclang.exe "$DIST_DIR/windows/qclang.exe"
else
    echo -e "${RED}❌ No Windows binary found${NC}"
    exit 1
fi

# 4. Create install scripts
echo -e "${YELLOW}📝 Creating install scripts...${NC}"

# Linux install script
cat > "$DIST_DIR/install-linux.sh" << 'LINUX_EOF'
#!/bin/bash
# QCLang Linux Installer

set -e

echo "Installing QCLang Quantum Compiler..."

# Check if running as root
if [ "$EUID" -ne 0 ]; then 
    echo "This script requires root privileges. Please run with sudo."
    exit 1
fi

# Copy binary
cp qclang /usr/local/bin/qclang
chmod +x /usr/local/bin/qclang

echo "✅ QCLang installed successfully!"
echo ""
echo "Usage:"
echo "  qclang compile <file.qc>    # Compile a quantum program"
echo "  qclang --help               # Show all commands"
echo ""
echo "Examples are in the examples/ directory."
LINUX_EOF
chmod +x "$DIST_DIR/install-linux.sh"

# Windows install script
cat > "$DIST_DIR/install-windows.bat" << 'WIN_EOF'
@echo off
REM QCLang Windows Installer
echo Installing QCLang Quantum Compiler...
echo.

REM Check for admin rights
net session >nul 2>&1
if %errorLevel% neq 0 (
    echo Error: This script requires administrator privileges.
    echo Please right-click and "Run as administrator".
    pause
    exit /b 1
)

REM Create installation directory
if not exist "C:\Program Files\QCLang" mkdir "C:\Program Files\QCLang"

REM Copy executable
copy qclang.exe "C:\Program Files\QCLang\qclang.exe"

REM Add to system PATH
setx PATH "%PATH%;C:\Program Files\QCLang" /M

echo ✅ QCLang installed successfully!
echo.
echo Usage:
echo   qclang compile ^<file.qc^>    REM Compile a quantum program
echo   qclang --help                 REM Show all commands
echo.
echo Please restart your terminal for PATH changes to take effect.
pause
WIN_EOF

# 5. Create README
cat > "$DIST_DIR/README.md" << 'README_EOF'
# QCLang Quantum Compiler v0.4.1

## Quick Start

### Linux
```bash
# Make the installer executable
chmod +x install-linux.sh

# Run as root
sudo ./install-linux.sh