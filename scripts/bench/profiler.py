#!/usr/bin/env python3
"""Profiler · deep timing + cold install

Expanded merge of: deep_profiler.py, profile_cold_install.py
Usage: python3 profiler.py [deep|cold]
Default mode: deep
"""
from __future__ import annotations

import sys

# ── from deep_profiler.py ─────────────────────────────────────────────
def run_deep(argv: list[str] | None = None) -> None:
    """Original script: deep_profiler.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Deep Profiler - Compare JHOL vs Bun to find exact bottlenecks
    """

    import subprocess
    import time
    import json
    import tempfile
    from pathlib import Path

    def profile_with_timing(cmd, label):
        """Profile a command with detailed timing"""
        print(f"\n{'='*60}")
        print(f"Profiling: {label}")
        print(f"Command: {' '.join(cmd)}")
        print('='*60)

        # Run with time measurement
        start = time.perf_counter()
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
        )
        elapsed = time.perf_counter() - start

        print(f"Total Time: {elapsed:.3f}s")
        print(f"Return Code: {result.returncode}")

        # Parse any timing info from output
        for line in result.stderr.split('\n') + result.stdout.split('\n'):
            if 'ms' in line.lower() or 'stage' in line.lower() or 'profile' in line.lower():
                print(f"  {line}")

        return elapsed, result

    def main():
        print("="*60)
        print("JHOL vs Bun - Deep Bottleneck Analysis")
        print("="*60)

        packages = ["lodash", "axios", "chalk"]

        with tempfile.TemporaryDirectory() as tmpdir:
            # Test JHOL
            jhol_dir = Path(tmpdir) / "jhol-test"
            jhol_dir.mkdir()
            jhol_cache = jhol_dir / ".cache"
            jhol_cache.mkdir()

            # Create package.json
            pkg_json = {
                "name": "test",
                "version": "1.0.0",
                "dependencies": {pkg: "^4.0.0" for pkg in packages}
            }
            (jhol_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

            # Profile JHOL
            jhol_time, jhol_result = profile_with_timing(
                ["/Users/bhuvanprakash/j/jhol/target/release/jhol", "install"] + packages,
                "JHOL Cold Install"
            )

            # Test Bun
            bun_dir = Path(tmpdir) / "bun-test"
            bun_dir.mkdir()
            (bun_dir / "package.json").write_text(json.dumps(pkg_json, indent=2))

            bun_time, bun_result = profile_with_timing(
                ["bun", "install"],
                "Bun Cold Install"
            )

            # Analysis
            print("\n" + "="*60)
            print("BOTTLENECK ANALYSIS")
            print("="*60)

            print(f"\nJHOL: {jhol_time:.3f}s")
            print(f"Bun:  {bun_time:.3f}s")
            print(f"Gap:  {jhol_time - bun_time:.3f}s ({jhol_time/bun_time:.1f}x slower)")

            # Break down the gap
            print("\n📊 Time Breakdown:")

            # Check JHOL profiling output
            jhol_stages = {}
            for line in jhol_result.stderr.split('\n'):
                if 'stage=' in line and 'delta_ms=' in line:
                    # Parse: [timestamp] [jhol-profile] stage=xxx delta_ms=yyy total_ms=zzz
                    parts = line.split()
                    for part in parts:
                        if part.startswith('stage='):
                            stage = part.split('=')[1]
                        elif part.startswith('delta_ms='):
                            delta = int(part.split('=')[1])
                            jhol_stages[stage] = delta

            if jhol_stages:
                print("\nJHOL Internal Stages:")
                for stage, ms in sorted(jhol_stages.items(), key=lambda x: -x[1]):
                    print(f"  {stage}: {ms}ms ({ms/1000:.3f}s)")

            # Estimate Bun's stages (based on known architecture)
            print("\nBun Estimated Stages (based on architecture):")
            bun_resolution = 0.01  # Bun uses cached metadata
            bun_download = bun_time * 0.6  # 60% download
            bun_extract = bun_time * 0.3  # 30% extract
            bun_other = bun_time * 0.1  # 10% overhead

            print(f"  Resolution: ~{bun_resolution*1000:.0f}ms (cached)")
            print(f"  Download:   ~{bun_download*1000:.0f}ms")
            print(f"  Extract:    ~{bun_extract*1000:.0f}ms")
            print(f"  Other:      ~{bun_other*1000:.0f}ms")

            # Identify bottlenecks
            print("\n🔍 IDENTIFIED BOTTLENECKS:")
            print("-"*60)

            bottleneck_num = 1

            # Compare resolution
            jhol_resolution = sum(jhol_stages.get('resolve_cold_specs', 0) for _ in range(1)) / 1000
            if jhol_resolution > bun_resolution * 2:
                print(f"\n{bottleneck_num}. RESOLUTION TOO SLOW")
                print(f"   JHOL: {jhol_resolution:.3f}s vs Bun: {bun_resolution:.3f}s")
                print(f"   Gap: {jhol_resolution - bun_resolution:.3f}s")
                print(f"   → Fix: Improve caching, use binary index")
                bottleneck_num += 1

            # Compare download
            jhol_download = sum(jhol_stages.get('download_unpack_install_cold', 0) for _ in range(1)) / 1000
            if jhol_download > bun_download * 1.5:
                print(f"\n{bottleneck_num}. DOWNLOAD TOO SLOW")
                print(f"   JHOL: {jhol_download:.3f}s vs Bun: {bun_download:.3f}s")
                print(f"   Gap: {jhol_download - bun_download:.3f}s")
                print(f"   → Fix: Increase concurrency, use HTTP/2, binary packages")
                bottleneck_num += 1

            # Check for missing optimizations
            print(f"\n{bottleneck_num}. MISSING OPTIMIZATIONS:")
            if "binary" not in jhol_result.stdout.lower():
                print(f"   ❌ No binary package cache (Bun uses pre-built binaries)")
            if "http/2" not in jhol_result.stderr.lower():
                print(f"   ❌ HTTP/2 not enabled (Bun uses custom HTTP stack)")
            if "io_uring" not in jhol_result.stderr.lower():
                print(f"   ❌ No io_uring (Bun uses io_uring on Linux)")

            # Recommendations
            print("\n" + "="*60)
            print("🎯 PRIORITY FIXES (in order)")
            print("="*60)
            print("1. Binary Package Cache - Pre-built binaries (3x faster)")
            print("2. HTTP/2 Multiplexing - Better connection reuse (30% faster)")
            print("3. Increase Download Concurrency - More parallel downloads")
            print("4. Streaming Extraction - Download + extract concurrently")
            print("5. Binary Index - Embed package metadata in binary")

    main()


# ── from profile_cold_install.py ─────────────────────────────────────────────
def run_cold(argv: list[str] | None = None) -> None:
    """Original script: profile_cold_install.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Cold Install Profiler - Find exact bottlenecks
    """

    import subprocess
    import time
    import json
    import tempfile
    import shutil
    from pathlib import Path
    from datetime import datetime

    def profile_cold_install():
        """Profile each stage of cold install"""

        print("="*60)
        print("JHOL Cold Install Profiler")
        print("="*60)

        # Test packages
        packages = ["lodash", "axios", "chalk"]

        with tempfile.TemporaryDirectory() as tmpdir:
            test_dir = Path(tmpdir) / "test"
            test_dir.mkdir()
            cache_dir = test_dir / ".cache"
            cache_dir.mkdir()

            # Create package.json
            package_json = {
                "name": "profile-test",
                "version": "1.0.0",
                "dependencies": {pkg: "^4.0.0" for pkg in packages}
            }
            (test_dir / "package.json").write_text(json.dumps(package_json, indent=2))

            env = {
                "JHOL_CACHE_DIR": str(cache_dir),
                "JHOL_QUIET": "0",  # Verbose output
                "JHOL_PROFILE_INSTALL": "1",  # Enable profiling
            }

            print(f"\n📦 Installing {len(packages)} packages (cold)...")
            print(f"   Packages: {', '.join(packages)}")
            print(f"   Directory: {test_dir}")
            print(f"   Cache: {cache_dir}")
            print()

            # Run install with profiling
            start = time.perf_counter()

            result = subprocess.run(
                ["/Users/bhuvanprakash/j/jhol/target/release/jhol", "install"] + packages,
                cwd=test_dir,
                env={**subprocess.os.environ, **env},
                capture_output=True,
                text=True,
            )

            elapsed = time.perf_counter() - start

            print(f"\n⏱️  Total Time: {elapsed:.3f}s")
            print(f"   Return Code: {result.returncode}")

            # Parse profiling output
            print("\n📊 Profiling Breakdown:")
            print("-" * 60)

            stages = []
            for line in result.stderr.split('\n'):
                if '[profile]' in line or '[jhol-profile]' in line:
                    print(f"   {line}")
                    stages.append(line)

            if not stages:
                print("   ⚠️  No profiling data found")
                print("   Enable profiling with: JHOL_PROFILE_INSTALL=1")

            # Analyze stdout for timing clues
            print("\n📈 Install Log:")
            print("-" * 60)
            for line in result.stdout.split('\n')[:20]:
                if line.strip():
                    print(f"   {line}")

            # Check for errors
            if result.returncode != 0:
                print("\n❌ Errors:")
                print("-" * 60)
                for line in result.stderr.split('\n'):
                    if 'error' in line.lower() or 'Error' in line:
                        print(f"   {line}")

            # Network timing estimation
            print("\n🌐 Network Analysis:")
            print("-" * 60)

            # Count HTTP requests from logs
            http_requests = result.stderr.lower().count('http') + result.stdout.lower().count('http')
            fetch_count = result.stderr.lower().count('fetch') + result.stdout.lower().count('fetch')
            download_count = result.stderr.lower().count('download') + result.stdout.lower().count('download')

            print(f"   HTTP mentions: {http_requests}")
            print(f"   Fetch mentions: {fetch_count}")
            print(f"   Download mentions: {download_count}")

            # Estimate network time (assuming ~100ms per request)
            estimated_network_time = (http_requests + fetch_count + download_count) * 0.1
            print(f"   Estimated network time: ~{estimated_network_time:.1f}s")

            # Disk I/O estimation
            print("\n💾 Disk I/O Analysis:")
            print("-" * 60)

            node_modules = test_dir / "node_modules"
            if node_modules.exists():
                total_size = sum(f.stat().st_size for f in node_modules.rglob('*') if f.is_file())
                file_count = len(list(node_modules.rglob('*')))
                print(f"   Files created: {file_count}")
                print(f"   Total size: {total_size / 1024 / 1024:.2f} MB")

                # Estimate disk write time (assuming 100 MB/s)
                estimated_disk_time = total_size / 1024 / 1024 / 100
                print(f"   Estimated disk write time: ~{estimated_disk_time:.2f}s")

            # Resolution time estimation
            print("\n🧮 Resolution Analysis:")
            print("-" * 60)
            resolve_mentions = result.stderr.lower().count('resolve') + result.stdout.lower().count('resolve')
            print(f"   Resolution mentions: {resolve_mentions}")
            print(f"   Estimated resolution time: ~0.1-0.5s (JAGR-3 is fast)")

            # Summary
            print("\n" + "="*60)
            print("🎯 BOTTLENECK ANALYSIS")
            print("="*60)

            if estimated_network_time > elapsed * 0.5:
                print(f"   🔴 NETWORK is the bottleneck ({estimated_network_time:.1f}s / {elapsed:.1f}s)")
                print("      → Need better connection pooling")
                print("      → Need HTTP/2 multiplexing")
                print("      → Need parallel downloads")
            elif 'disk' in result.stderr.lower() or file_count > 1000:
                print(f"   🟡 DISK I/O is a bottleneck ({file_count} files)")
                print("      → Need hard links (already implemented)")
                print("      → Need parallel extraction")
            else:
                print(f"   🟢 Resolution/Other ({elapsed - estimated_network_time:.1f}s)")
                print("      → Need better profiling")

            print("\n💡 RECOMMENDATIONS:")
            print("-" * 60)
            print("   1. Add detailed timing to each install stage")
            print("   2. Profile HTTP requests individually")
            print("   3. Measure disk I/O separately")
            print("   4. Compare with Bun's approach")

            # Save detailed log
            log_file = Path("profile-log.txt")
            with open(log_file, 'w') as f:
                f.write(f"Timestamp: {datetime.now().isoformat()}\n")
                f.write(f"Packages: {packages}\n")
                f.write(f"Total Time: {elapsed:.3f}s\n\n")
                f.write("STDOUT:\n")
                f.write(result.stdout)
                f.write("\n\nSTDERR:\n")
                f.write(result.stderr)

            print(f"\n📄 Detailed log saved to: {log_file}")

    profile_cold_install()


# ── dispatcher ──────────────────────────────────────────────────
_MODES = {
    'deep': run_deep,
    'cold': run_cold,
}

def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in _MODES:
        mode = argv.pop(0)
    else:
        mode = 'deep'
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
