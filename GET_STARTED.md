# **Enhanced Getting Started with Jhol**

Welcome to **Jhol** – a fast, and offline-friendly package manager designed to provide a seamless experience for developers.

Jhol offers:
✅ **Faster Package Installation** – Uses caching to speed up repeated installations.  
✅ **Offline Support** – Installs previously downloaded packages without an internet connection.  
✅ **Dependency Management** – Detects and fixes broken dependencies automatically.  
✅ **Simple & Intuitive CLI** – Just like `npm` and `yarn`, but optimized for speed and efficiency.  

---
## **🔹 Installation**
### **Linux & macOS**
To install Jhol, use the following command:
```sh
cargo install --git https://github.com/bhuvanprakash/jhol
```
This will download the latest version of Jhol and compile it from source.

### **Windows**
If you're on **Windows**, follow these steps:
1. Install Rust: [https://rustup.rs](https://rustup.rs)
2. Clone the repository and build:
   ```powershell
   git clone https://github.com/bhuvanprakash/jhol.git
   cd jhol
   cargo build --release
   ```
3. Move the compiled binary to a system-wide location:
   ```powershell
   mkdir "C:\Program Files\Jhol"
   copy .\target\release\jhol.exe "C:\Program Files\Jhol\"
   ```
4. Add Jhol to your **Windows PATH**:
   ```powershell
   setx PATH "$env:PATH;C:\Program Files\Jhol\"
   ```
5. Restart your terminal and verify the installation:
   ```powershell
   jhol --version
   ```

Or run the batch file (as Administrator): `install_jhol.bat`

To install the binary to your PATH so you can run `jhol` from anywhere:
```sh
jhol global-install
```

---
## **🔹 Usage**
Jhol works like `npm` and `yarn`, but is optimized for better caching and offline support.

### **Installing a Package**
To install a package, use:
```sh
jhol install <package-name>
```
Example:
```sh
jhol install axios
```
This will:
- Check if the package is cached.
- If cached, install it **without downloading**.
- If not, fetch it from **NPM** and cache it for future offline use.

#### **Installing Specific Versions**
You can install specific versions like this:
```sh
jhol install react@17.0.0 react@18.0.0
```
Jhol will fetch and cache both versions.

---
### **Checking and Fixing Dependencies**
Jhol comes with an **intelligent dependency checker**.

#### **Check for Issues**
```sh
jhol doctor
```
This scans your project for missing or outdated dependencies.

#### **Automatically Fix Issues**
```sh
jhol doctor --fix
```
This updates outdated packages and installs missing ones.

---
## **🔹 How Does Jhol Work?**
1. **Cache First** – If a package exists in Jhol’s cache, it installs instantly.
2. **Fallback to NPM** – If not found, Jhol fetches the package from **NPM**.
3. **Offline Mode** – If the network is offline, Jhol still installs from cache.
4. **Dependency Fixing** – Detects outdated or broken dependencies and fixes them.

---
## **🔹 Advanced Features**
### **Managing the Cache**
```sh
jhol cache list    # List cached packages
jhol cache clean   # Remove all cached tarballs (forces fresh fetch next time)
```
Or manually: `rm -rf ~/.jhol-cache` (Unix) / `rd /s /q %USERPROFILE%\.jhol-cache` (Windows).

### **Logging**
Jhol maintains logs of package installations in:
```
~/.jhol-cache/logs.txt
```
You can check past installations and errors here.

---
## **🔹 Documentation**
For more detailed documentation, please refer to:
- **[README.md](README.md)**
- **Jhol GitHub Repository**: [https://github.com/bhuvanprakash/jhol](https://github.com/bhuvanprakash/jhol)

---
## **🔹 Conclusion**
Jhol is a **faster, smarter, and offline-friendly package manager** that enhances the developer experience.

🔧 **Start using Jhol today** and make package management faster & smoother!