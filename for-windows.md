## **How to Run Jhol on Windows (Step-by-Step Guide)**  

Since **Jhol** is built in **Rust**, you must compile it before using it on Windows. Follow these steps to set it up properly.

---

### **🔹 1. Install Rust (If Not Installed)**
Jhol requires Rust to be installed. If you don’t have Rust installed, do this:
1. **Download Rust**:  
   - Visit: [https://rustup.rs/](https://rustup.rs/)
   - Click **"Install Rust"** and follow the instructions.
2. **Verify Installation**:
   ```powershell
   rustc --version
   cargo --version
   ```
---

### **🔹 2. Clone or Download Jhol Source Code**
1. Open **PowerShell** or **Command Prompt** (`cmd`).
2. Clone the Jhol repository (or extract it if you downloaded a ZIP):
   ```powershell
   git clone https://github.com/bhuvanprakash/jhol.git
   cd jhol
   ```
---

### **🔹 3. Build Jhol for Windows**
1. **Run the following command** in the Jhol directory:
   ```powershell
   cargo build --release
   ```
   This will generate a compiled executable at:
   ```
   .\target\release\jhol.exe
   ```
2. **Verify the Build**:
   ```powershell
   .\target\release\jhol.exe --version
   ```
---

### **🔹 4. Install Jhol System-Wide on Windows**
So that you can run `jhol install axios` anywhere, follow these steps:

#### **Method 1: Use `install_jhol.bat` (Automatic)**
1. Create a file named **`install_jhol.bat`** in the **`jhol-free`** directory.

2. **Run the script**:
   - **Right-click** `install_jhol.bat` → **Run as Administrator**.
   - It will **copy `jhol.exe` to `C:\Program Files\Jhol\` and add it to the system PATH**.
   - Restart your terminal.

---

#### **Method 2: Manually Add to PATH (Alternative)**
1. Move `jhol.exe` to `C:\Program Files\Jhol\`
   ```powershell
   mkdir "C:\Program Files\Jhol"
   copy .\target\release\jhol.exe "C:\Program Files\Jhol\"
   ```
2. Add `C:\Program Files\Jhol\` to **Windows PATH**:
   ```powershell
   setx PATH "$env:PATH;C:\Program Files\Jhol\"
   ```
3. Restart the terminal.

---

### **🔹 5. Run Jhol Anywhere**
Now you can **use Jhol globally**:
```powershell
jhol --version
jhol install axios
jhol doctor --fix
```
**Now Jhol works like `npm` or `yarn` on Windows!** 🚀