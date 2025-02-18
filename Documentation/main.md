### **Jhol - A Faster, Decentralized Package Manager (Free Version)**
**Version**: 1.0.0  
**Author**: Bhuvan Prakash  
**License**: Jhol-License 1.0  (Unlicensed) 

---

# **Table of Contents**
1. [Introduction](#-introduction)  
2. [Installation & Setup](#-installation--setup)  
3. [Basic Usage](#-basic-usage)  
4. [Advanced Features](#-advanced-features)  
5. [Configuration](#-configuration)  
6. [Package Management](#-package-management)  
7. [Troubleshooting](#-troubleshooting)  
8. [Contributing](#-contributing)  
9. [Security Considerations](#-security-considerations)  
10. [FAQs](#-faqs)  

---

## **Introduction**
Jhol is a lightweight, decentralized package manager that serves as an alternative to **NPM** and **Yarn**.  
It provides **fast, cached installations** while falling back to NPM when necessary.

### **🔹 Key Features**
✅ **Local Caching** → Saves installed packages for **offline use**  
✅ **NPM Fallback** → Installs missing packages using NPM  
✅ **Dependency Fixing** → Automatically detects and fixes issues  
✅ **Multiple Package Support** → Install multiple packages in one command  
✅ **Fast Performance** → Minimizes redundant package downloads  

---

## **nstallation & Setup**
### **Prerequisites**
- **Rust & Cargo Installed**: Jhol is built in Rust, so you'll need Rust to compile it.  
  Install Rust using:
  ```sh
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  ```
- **Node.js & NPM**: Required for fallback installations.  
  Install using:
  ```sh
  sudo apt install nodejs npm  # Debian/Ubuntu
  brew install node            # macOS (Homebrew)
  ```
- **Git**: Needed for downloading Jhol source code.
  ```sh
  sudo apt install git  # Debian/Ubuntu
  ```

### **Cloning & Building Jhol**
1. Clone the repository:
   ```sh
   git clone https://github.com/bhuvanprakash/jhol.git
   cd jhol
   ```
2. Compile the project:
   ```sh
   cargo build --release
   ```
3. Run Jhol to check if it's installed:
   ```sh
   ./target/release/jhol --help
   ```

---

## **Basic Usage**
### **Installing Packages**
```sh
./target/release/jhol install <package>
```
Example:
```sh
./target/release/jhol install lodash axios express
```
**How it works**:
1. **Checks Cache**: If the package exists in `~/.jhol-cache`, it installs from there.
2. **NPM Fallback**: If missing, it fetches from NPM and stores it in the cache.

### **Checking & Fixing Dependencies**
```sh
./target/release/jhol doctor --fix
```
**What it does**:
- Scans `package.json`
- Identifies outdated/missing dependencies
- Fixes them automatically

### **Viewing Cached Packages**
```sh
ls -lah ~/.jhol-cache
cat ~/.jhol-cache/logs.txt
```

### **Clearing Cache**
To remove all cached packages:
```sh
rm -rf ~/.jhol-cache
```

---

## **Advanced Features**
### **Installing Specific Versions**
```sh
./target/release/jhol install react@18.0.0
```
This installs **React 18.0.0**, and caches it for offline use.

### **Installing Multiple Versions**
```sh
./target/release/jhol install react@18.0.0 react@17.0.0
```
Jhol caches both versions separately.

### **📌 Offline Mode**
You can disable your network (`nmcli networking off` on Linux) and still install cached packages:
```sh
./target/release/jhol install lodash
```
It will install **lodash from cache**, proving **offline capabilities**.

---

## **⚙️ Configuration**
Jhol automatically stores packages in:
```
~/.jhol-cache/
```
This can be modified by changing `utils.rs`:
```rust
pub fn get_cache_dir() -> String {
    env::var("HOME").unwrap_or_else(|_| ".".to_string()) + "/.jhol-cache"
}
```

---

## **Package Management**
### **Checking Installed Packages**
```sh
ls ~/.jhol-cache
```

### **Uninstalling Packages**
Currently, Jhol does not support uninstallation. You must manually remove cache:
```sh
rm ~/.jhol-cache/<package>
```

---

## **🛠 Troubleshooting**
| Issue | Solution |
|---|---|
| `jhol: command not found` | Run `cargo build --release` inside the `jhol` directory |
| `Permission denied while removing ~/.jhol-cache` | Use `sudo rm -rf ~/.jhol-cache` |
| `Failed to install package` | Ensure **NPM is installed** by running `npm --version` |
| `Jhol hangs on installation` | Restart terminal and retry |

---

## **Contributing**
### **How to Contribute?**
1. **Fork the Repo** on GitHub
2. **Clone Locally**
   ```sh
   git clone https://github.com/bhuvanprakash/jhol.git
   ```
3. **Make Changes & Test**
4. **Submit a Pull Request**

### **Code Guidelines**
- Use **Rust best practices**
- Ensure **error handling** is robust
- Maintain **code readability**
- Test before pushing updates

---

## **Security Considerations**
- Jhol does **not** verify package authenticity.
- Cached packages could be **modified manually**.
- Future versions will include **package verification**.

---

## **FAQs**
### **Q1: How is Jhol different from NPM or Yarn?**
**Jhol caches installations**, allowing offline installs, unlike NPM/Yarn.

### **Q2: Can I use Jhol globally like NPM?**
No, Jhol is currently a **local package manager**.

### **Q3: What if a package is missing from cache?**
Jhol **automatically fetches it from NPM**.

### **Q4: How does Jhol handle updates?**
Use `jhol doctor --fix` to update outdated dependencies.

---

## **Summary**
| Feature | Status |
|---|---|
| Local caching | ✅ |
| NPM Fallback | ✅ |
| Offline Mode | ✅ |
| Dependency Fixing | ✅ |
| Global Install | ❌ (Planned) |
| Security Verification | ❌ (Planned) |

---

## ** Final Thoughts**
Jhol is a **lightweight, offline-friendly package manager** that speeds up installations and ensures dependencies are always available.  

**Want more features?** Submit a request in GitHub issues! 