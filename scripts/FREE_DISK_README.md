# Freeing More Disk Space on macOS

Use the script and steps below to reclaim space. We already freed **~16GB** (Cursor backup + logs + caches).

## Run the script (safe, no sudo)

```bash
chmod +x scripts/free-mac-disk-space.sh
./scripts/free-mac-disk-space.sh
```

## Free ~30GB more (optional)

### Msty local models — **~30GB**
If you use **Msty** and don’t need local AI models, you can delete them (you can re-download later in Msty):

```bash
rm -rf ~/Library/Application\ Support/Msty/models/*
```
Or delete the whole folder: `~/Library/Application Support/Msty/models`

### Docker Desktop data — **~34GB**
Your Mac has **Docker Desktop** data at `~/Library/Containers/com.docker.docker` (~34GB). To reclaim it:

- **If you use Docker Desktop:** Open Docker Desktop → **Settings (gear)** → **Troubleshoot** → **Clean / Purge data** or **Reset to factory defaults** (removes all images/containers/volumes).
- **If you only use Colima:** You can remove Docker Desktop and its data:
  ```bash
  # Quit Docker Desktop first, then:
  rm -rf ~/Library/Containers/com.docker.docker
  ```
  Uninstall the app from Applications if you don’t need it.

---

## "System Data" and macOS – other options

### 1. **Time Machine local snapshots** (often 10–50GB)
- List: `tmutil listlocalsnapshots /`
- Delete old ones: `tmutil deletelocalsnapshots <date>`  
  Or: System Settings → General → Storage → Manage → enable "Optimize Storage".

### 2. **iOS/iPadOS device backups**
- System Settings → General → Storage → Manage → delete old device backups.

### 3. **System caches (needs Admin)**
```bash
sudo rm -rf /Library/Caches/*
sudo rm -rf /private/var/folders/*/*/C/*
```
Restart after.

### 4. **Xcode**
- Xcode → Settings → Locations → Derived Data → Delete
- Xcode → Settings → Platforms → remove old simulators.

### 5. **Large files**
```bash
find ~ -type f -size +500M 2>/dev/null
```

### 6. **Optimize Storage (built-in)**
- System Settings → General → Storage → Manage → turn on "Optimize Storage".
