# Running Jhol on Windows

This guide explains how to build, install, and run Jhol on Windows.

---

## 1) Install prerequisites

Install Rust and Cargo from [https://rustup.rs/](https://rustup.rs/), then verify:

```powershell
rustc --version
cargo --version
```

---

## 2) Clone the repository

```powershell
git clone https://github.com/bhuvanprakash/jhol.git
cd jhol
```

---

## 3) Build Jhol

```powershell
cargo build --release
```

The executable will be generated at:

```text
.\target\release\jhol.exe
```

Verify it:

```powershell
.\target\release\jhol.exe --version
```

---

## 4) Install system-wide

### Option A: Automatic (recommended)

Run `install_jhol.bat` as Administrator. It copies `jhol.exe` to `C:\Program Files\Jhol\` and updates PATH.

### Option B: Manual

```powershell
mkdir "C:\Program Files\Jhol"
copy .\target\release\jhol.exe "C:\Program Files\Jhol\"
setx PATH "$env:PATH;C:\Program Files\Jhol\"
```

Restart your terminal after updating PATH.

---

## 5) Use Jhol globally

```powershell
jhol --version
jhol install axios
jhol doctor --fix
```
