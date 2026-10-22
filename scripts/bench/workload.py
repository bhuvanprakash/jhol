#!/usr/bin/env python3
"""Workload · large graph + repeat learning

Expanded merge of: benchmark_large_workload.py, benchmark_repeat.py
Usage: python3 workload.py [large|repeat]
Default mode: large
"""
from __future__ import annotations

import sys

# ── from benchmark_large_workload.py ─────────────────────────────────────────────
def run_large(argv: list[str] | None = None) -> None:
    """Original script: benchmark_large_workload.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Large Workload Benchmark for JHOL
    Tests with 50+ packages to maximize CDCL and watched literals impact
    """

    import subprocess
    import json
    import time
    import os
    import sys
    from pathlib import Path
    from typing import Dict, List, Tuple
    import tempfile
    import shutil

    def create_large_package_json(size: str = "large") -> Dict:
        """Create package.json with many dependencies"""

        if size == "large":
            # 50+ dependencies - typical large project
            dependencies = {
                # React ecosystem
                "react": "^18.2.0",
                "react-dom": "^18.2.0",
                "react-router-dom": "^6.20.0",
                "@types/react": "^18.2.0",
                "@types/react-dom": "^18.2.0",

                # Build tools
                "typescript": "^5.3.0",
                "vite": "^5.0.0",
                "webpack": "^5.89.0",
                "webpack-cli": "^5.1.0",
                "babel-loader": "^9.1.0",
                "@babel/core": "^7.23.0",
                "@babel/preset-env": "^7.23.0",
                "@babel/preset-react": "^7.23.0",
                "@babel/preset-typescript": "^7.23.0",

                # Styling
                "tailwindcss": "^3.3.0",
                "postcss": "^8.4.0",
                "autoprefixer": "^10.4.0",
                "sass": "^1.69.0",
                "less": "^4.2.0",

                # Testing
                "vitest": "^1.0.0",
                "@testing-library/react": "^14.1.0",
                "@testing-library/jest-dom": "^6.1.0",
                "jsdom": "^23.0.0",
                "cypress": "^13.6.0",

                # Linting & Formatting
                "eslint": "^8.55.0",
                "@typescript-eslint/parser": "^6.13.0",
                "@typescript-eslint/eslint-plugin": "^6.13.0",
                "eslint-plugin-react": "^7.33.0",
                "eslint-plugin-react-hooks": "^4.6.0",
                "prettier": "^3.1.0",
                "eslint-config-prettier": "^9.1.0",

                # Utilities
                "lodash": "^4.17.21",
                "axios": "^1.6.0",
                "moment": "^2.29.0",
                "dayjs": "^1.11.0",
                "uuid": "^9.0.0",
                "classnames": "^2.3.0",

                # State management
                "zustand": "^4.4.0",
                "recoil": "^0.7.7",

                # HTTP & API
                "@tanstack/react-query": "^5.13.0",
                "graphql": "^16.8.0",
                "@apollo/client": "^3.8.0",

                # Forms
                "react-hook-form": "^7.49.0",
                "formik": "^2.4.0",
                "yup": "^1.3.0",
                "zod": "^3.22.0",

                # UI Libraries
                "@headlessui/react": "^1.7.0",
                "@heroicons/react": "^2.1.0",
                "radix-ui": "^1.0.0",

                # Dev tools
                "concurrently": "^8.2.0",
                "nodemon": "^3.0.0",
                "rimraf": "^5.0.0",
                "cross-env": "^7.0.0",
            }

            return {
                "name": "jhol-benchmark-large",
                "version": "1.0.0",
                "private": True,
                "dependencies": dependencies,
            }

        elif size == "xlarge":
            # 100+ dependencies - monorepo scale
            base = create_large_package_json("large")["dependencies"]

            # Add more packages
            extra = {
                "next": "^14.0.0",
                "express": "^4.18.0",
                "fastify": "^4.25.0",
                "nestjs": "^10.3.0",
                "prisma": "^5.7.0",
                "@prisma/client": "^5.7.0",
                "typeorm": "^0.3.0",
                "mongoose": "^8.0.0",
                "sequelize": "^6.35.0",
                "knex": "^3.1.0",
                "redis": "^4.6.0",
                "ioredis": "^5.3.0",
                "socket.io": "^4.7.0",
                "ws": "^8.14.0",
                "jsonwebtoken": "^9.0.0",
                "bcrypt": "^5.1.0",
                "passport": "^0.7.0",
                "helmet": "^7.1.0",
                "cors": "^2.8.0",
                "compression": "^1.7.0",
                "morgan": "^1.10.0",
                "dotenv": "^16.3.0",
                "config": "^3.3.0",
                "commander": "^11.1.0",
                "inquirer": "^9.2.0",
                "chalk": "^5.3.0",
                "ora": "^7.0.0",
                "listr2": "^7.0.0",
                "execa": "^8.0.0",
                "glob": "^10.3.0",
                "fast-glob": "^3.3.0",
                "fs-extra": "^11.2.0",
                "node-fetch": "^3.3.0",
                "got": "^13.0.0",
                "axios-retry": "^4.0.0",
                "pino": "^8.17.0",
                "winston": "^3.11.0",
                "bunyan": "^1.8.0",
                "debug": "^4.3.0",
                "node-fetch-native": "^1.4.0",
                "ofetch": "^1.3.0",
                "destr": "^2.0.0",
                "h3": "^1.9.0",
                "nitropack": "^2.8.0",
                "unbuild": "^2.0.0",
                "pkg": "^5.8.0",
                "ncc": "^0.38.0",
                "esbuild": "^0.19.0",
                "rollup": "^4.9.0",
                "vite-plugin-dts": "^3.6.0",
                "unplugin": "^1.5.0",
                "unplugin-vue-components": "^0.26.0",
                "unplugin-auto-import": "^0.17.0",
                "vue": "^3.4.0",
                "vue-router": "^4.2.0",
                "pinia": "^2.1.0",
                "nuxt": "^3.9.0",
                "@vueuse/core": "^10.7.0",
                "element-plus": "^2.4.0",
                "ant-design-vue": "^4.0.0",
                "quasar": "^2.14.0",
                "vuetify": "^3.4.0",
                "primevue": "^3.43.0",
                "naive-ui": "^2.36.0",
                "arco-design": "^2.55.0",
                "semi-ui": "^2.53.0",
                "tdesign": "^1.7.0",
                "varlet": "^3.2.0",
                "nutui": "^4.2.0",
                "devui": "^1.21.0",
                "vuepress": "^2.0.0",
                "vitepress": "^1.0.0",
                "docusaurus": "^3.0.0",
                "storybook": "^7.6.0",
                "ladle": "^3.0.0",
                "playwright": "^1.40.0",
                "puppeteer": "^21.6.0",
                "webdriverio": "^8.27.0",
                "testcafe": "^3.4.0",
                "jest": "^29.7.0",
                "mocha": "^10.2.0",
                "jasmine": "^5.1.0",
                "ava": "^6.1.0",
                "tape": "^5.7.0",
                "uvu": "^0.5.0",
                "karma": "^6.4.0",
                "protractor": "^7.0.0",
                "nightwatch": "^3.3.0",
                "codeceptjs": "^3.5.0",
                "detox": "^20.17.0",
                "appium": "^2.0.0",
                "maestro": "^1.0.0",
            }

            base.update(extra)

            return {
                "name": "jhol-benchmark-xlarge",
                "version": "1.0.0",
                "private": True,
                "dependencies": base,
            }

        else:
            raise ValueError(f"Unknown size: {size}")

    def run_benchmark(size: str, repeats: int = 3) -> Dict:
        """Run benchmark for given workload size"""

        print(f"\n{'='*60}")
        print(f"Running {size.upper()} workload benchmark ({repeats} repeats)")
        print(f"{'='*60}")

        package_json = create_large_package_json(size)
        package_count = len(package_json["dependencies"])
        print(f"Package count: {package_count}")

        results = {
            "size": size,
            "package_count": package_count,
            "cold_installs": [],
            "warm_installs": [],
            "offline_installs": [],
        }

        with tempfile.TemporaryDirectory() as tmpdir:
            test_dir = Path(tmpdir) / "test"
            test_dir.mkdir()

            # Write package.json
            with open(test_dir / "package.json", "w") as f:
                json.dump(package_json, f, indent=2)

            env = os.environ.copy()
            env["JHOL_CACHE_DIR"] = str(test_dir / ".cache")
            env["JHOL_QUIET"] = "1"

            jhol_bin = Path.cwd() / "target/release/jhol"

            # Cold installs
            print(f"\n📦 Running {repeats} cold installs...")
            for i in range(repeats):
                # Clean cache and node_modules
                cache_dir = test_dir / ".cache"
                if cache_dir.exists():
                    shutil.rmtree(cache_dir)
                cache_dir.mkdir()

                node_modules = test_dir / "node_modules"
                if node_modules.exists():
                    shutil.rmtree(node_modules)

                start = time.perf_counter()
                result = subprocess.run(
                    [str(jhol_bin), "install", "--native-only", "-q"],
                    cwd=test_dir,
                    env=env,
                    capture_output=True,
                    text=True,
                )
                elapsed = time.perf_counter() - start

                if result.returncode == 0:
                    results["cold_installs"].append(elapsed)
                    print(f"  Run {i+1}: {elapsed:.2f}s ✅")
                else:
                    print(f"  Run {i+1}: FAILED ❌")
                    print(f"    stderr: {result.stderr[:200]}")

            # Prime cache for warm installs
            print(f"\n🔥 Priming cache for warm installs...")
            node_modules = test_dir / "node_modules"
            if node_modules.exists():
                shutil.rmtree(node_modules)

            subprocess.run(
                [str(jhol_bin), "install", "--native-only", "-q"],
                cwd=test_dir,
                env=env,
                capture_output=True,
                text=True,
            )

            # Warm installs
            print(f"\n♨️  Running {repeats} warm installs...")
            for i in range(repeats):
                node_modules = test_dir / "node_modules"
                if node_modules.exists():
                    shutil.rmtree(node_modules)

                start = time.perf_counter()
                result = subprocess.run(
                    [str(jhol_bin), "install", "--native-only", "-q"],
                    cwd=test_dir,
                    env=env,
                    capture_output=True,
                    text=True,
                )
                elapsed = time.perf_counter() - start

                if result.returncode == 0:
                    results["warm_installs"].append(elapsed)
                    print(f"  Run {i+1}: {elapsed:.2f}s ✅")
                else:
                    print(f"  Run {i+1}: FAILED ❌")

            # Offline installs
            print(f"\n📴 Running {repeats} offline installs...")
            for i in range(repeats):
                node_modules = test_dir / "node_modules"
                if node_modules.exists():
                    shutil.rmtree(node_modules)

                start = time.perf_counter()
                result = subprocess.run(
                    [str(jhol_bin), "install", "--native-only", "--offline", "-q"],
                    cwd=test_dir,
                    env=env,
                    capture_output=True,
                    text=True,
                )
                elapsed = time.perf_counter() - start

                if result.returncode == 0:
                    results["offline_installs"].append(elapsed)
                    print(f"  Run {i+1}: {elapsed:.2f}s ✅")
                else:
                    print(f"  Run {i+1}: FAILED ❌")

        # Calculate statistics
        import statistics

        for key in ["cold_installs", "warm_installs", "offline_installs"]:
            values = results[key]
            if values:
                results[f"{key}_avg"] = statistics.mean(values)
                results[f"{key}_median"] = statistics.median(values)
                results[f"{key}_min"] = min(values)
                results[f"{key}_max"] = max(values)
                if len(values) > 1:
                    results[f"{key}_stdev"] = statistics.stdev(values)
                else:
                    results[f"{key}_stdev"] = 0

        return results

    def main():
        print("🚀 JHOL Large Workload Benchmark")
        print("Testing CDCL and Watched Literals impact on 50+ package graphs")

        results = []

        # Run large workload (50+ packages)
        results.append(run_benchmark("large", repeats=3))

        # Run xlarge workload (100+ packages) if requested
        if len(sys.argv) > 1 and sys.argv[1] == "--xlarge":
            results.append(run_benchmark("xlarge", repeats=2))

        # Print summary
        print("\n" + "="*60)
        print("📊 BENCHMARK SUMMARY")
        print("="*60)

        for result in results:
            print(f"\n{result['size'].upper()} ({result['package_count']} packages):")
            print(f"  Cold Install:")
            print(f"    Average: {result.get('cold_installs_avg', 'N/A'):.2f}s" if result.get('cold_installs_avg') else "    Average: N/A")
            print(f"    Median:  {result.get('cold_installs_median', 'N/A'):.2f}s" if result.get('cold_installs_median') else "    Median:  N/A")
            print(f"    Best:    {result.get('cold_installs_min', 'N/A'):.2f}s" if result.get('cold_installs_min') else "    Best:    N/A")
            print(f"  Warm Install:")
            print(f"    Average: {result.get('warm_installs_avg', 'N/A'):.2f}s" if result.get('warm_installs_avg') else "    Average: N/A")
            print(f"    Median:  {result.get('warm_installs_median', 'N/A'):.2f}s" if result.get('warm_installs_median') else "    Median:  N/A")
            print(f"  Offline Install:")
            print(f"    Average: {result.get('offline_installs_avg', 'N/A'):.2f}s" if result.get('offline_installs_avg') else "    Average: N/A")
            print(f"    Median:  {result.get('offline_installs_median', 'N/A'):.2f}s" if result.get('offline_installs_median') else "    Median:  N/A")

        # Save results
        with open("benchmark-large-workload.json", "w") as f:
            json.dump(results, f, indent=2)

        print(f"\n💾 Results saved to: benchmark-large-workload.json")
        print("\n✅ Benchmark completed!")

    main()


# ── from benchmark_repeat.py ─────────────────────────────────────────────
def run_repeat(argv: list[str] | None = None) -> None:
    """Original script: benchmark_repeat.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Repeat Benchmark for JHOL
    Tests persistent conflict clause benefits by running same workload multiple times
    """

    import subprocess
    import json
    import time
    import os
    from pathlib import Path
    import tempfile
    import shutil

    def create_test_package_json() -> dict:
        """Create a package.json with dependencies that trigger conflict learning"""
        return {
            "name": "jhol-repeat-benchmark",
            "version": "1.0.0",
            "private": True,
            "dependencies": {
                # React ecosystem with peer dependencies
                "react": "^18.2.0",
                "react-dom": "^18.2.0",
                "react-router-dom": "^6.20.0",
                "@types/react": "^18.2.0",

                # Build tools
                "typescript": "^5.3.0",
                "vite": "^5.0.0",

                # Styling
                "tailwindcss": "^3.3.0",

                # Testing
                "vitest": "^1.0.0",
                "@testing-library/react": "^14.1.0",

                # Linting
                "eslint": "^8.55.0",
                "@typescript-eslint/parser": "^6.13.0",
                "eslint-plugin-react": "^7.33.0",

                # Utilities
                "lodash": "^4.17.21",
                "axios": "^1.6.0",

                # State management
                "zustand": "^4.4.0",

                # HTTP
                "@tanstack/react-query": "^5.13.0",

                # Forms
                "react-hook-form": "^7.49.0",
                "zod": "^3.22.0",
            },
        }

    def run_repeat_benchmark(repeats: int = 10) -> dict:
        """Run same workload multiple times to measure cache benefits"""

        print(f"\n{'='*60}")
        print(f"Running REPEAT benchmark ({repeats} iterations)")
        print(f"{'='*60}")

        package_json = create_test_package_json()
        package_count = len(package_json["dependencies"])
        print(f"Package count: {package_count}")

        results = {
            "install_times": [],
            "cache_hits": [],
        }

        with tempfile.TemporaryDirectory() as tmpdir:
            test_dir = Path(tmpdir) / "test"
            test_dir.mkdir()

            # Write package.json
            with open(test_dir / "package.json", "w") as f:
                json.dump(package_json, f, indent=2)

            env = os.environ.copy()
            cache_dir = test_dir / ".cache"
            env["JHOL_CACHE_DIR"] = str(cache_dir)
            env["JHOL_QUIET"] = "1"

            jhol_bin = Path.cwd() / "target/release/jhol"

            print(f"\n📦 Running {repeats} consecutive installs (same cache)...")
            for i in range(repeats):
                # Don't clean cache between runs - this tests persistent benefits
                node_modules = test_dir / "node_modules"
                if node_modules.exists():
                    shutil.rmtree(node_modules)

                start = time.perf_counter()
                result = subprocess.run(
                    [str(jhol_bin), "install", "--native-only", "-q"],
                    cwd=test_dir,
                    env=env,
                    capture_output=True,
                    text=True,
                )
                elapsed = time.perf_counter() - start

                if result.returncode == 0:
                    results["install_times"].append(elapsed)

                    # Check for cache-related messages in stderr
                    stderr_lower = result.stderr.lower()
                    cache_hit = "cache" in stderr_lower or "cached" in stderr_lower
                    results["cache_hits"].append(cache_hit)

                    status = "✅"
                    if i == 0:
                        status += " (cold)"
                    elif i < 3:
                        status += " (warming)"
                    else:
                        status += " (hot)"

                    print(f"  Run {i+1:2d}: {elapsed:.2f}s {status}")
                else:
                    print(f"  Run {i+1:2d}: FAILED ❌")
                    results["install_times"].append(None)

        # Calculate statistics
        import statistics

        valid_times = [t for t in results["install_times"] if t is not None]

        if valid_times:
            results["avg"] = statistics.mean(valid_times)
            results["median"] = statistics.median(valid_times)
            results["min"] = min(valid_times)
            results["max"] = max(valid_times)
            results["stdev"] = statistics.stdev(valid_times) if len(valid_times) > 1 else 0

            # Calculate improvement from first to last
            if len(valid_times) >= 2:
                first_3_avg = statistics.mean(valid_times[:3])
                last_3_avg = statistics.mean(valid_times[-3:])
                improvement = (first_3_avg - last_3_avg) / first_3_avg * 100
                results["improvement_percent"] = improvement
                results["first_3_avg"] = first_3_avg
                results["last_3_avg"] = last_3_avg

        return results

    def main():
        print("🚀 JHOL Repeat Benchmark")
        print("Testing persistent conflict clause and cache benefits")

        results = run_repeat_benchmark(repeats=10)

        # Print summary
        print("\n" + "="*60)
        print("📊 BENCHMARK SUMMARY")
        print("="*60)

        print(f"\nInstall Times:")
        print(f"  Average:    {results.get('avg', 'N/A'):.2f}s")
        print(f"  Median:     {results.get('median', 'N/A'):.2f}s")
        print(f"  Best:       {results.get('min', 'N/A'):.2f}s")
        print(f"  Worst:      {results.get('max', 'N/A'):.2f}s")
        print(f"  Std Dev:    {results.get('stdev', 'N/A'):.2f}s")

        if "improvement_percent" in results:
            print(f"\n📈 Performance Improvement:")
            print(f"  First 3 runs avg:  {results['first_3_avg']:.2f}s")
            print(f"  Last 3 runs avg:   {results['last_3_avg']:.2f}s")
            print(f"  Improvement:       {results['improvement_percent']:.1f}% faster")

        # Save results
        with open("benchmark-repeat.json", "w") as f:
            json.dump(results, f, indent=2)

        print(f"\n💾 Results saved to: benchmark-repeat.json")
        print("\n✅ Benchmark completed!")

    main()


# ── dispatcher ──────────────────────────────────────────────────
_MODES = {
    'large': run_large,
    'repeat': run_repeat,
}

def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in _MODES:
        mode = argv.pop(0)
    else:
        mode = 'large'
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
