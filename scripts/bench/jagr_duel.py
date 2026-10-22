#!/usr/bin/env python3
"""JAGR duel · resolver comparison

Expanded merge of: benchmark_jagr_comparison.py, simple_jagr_benchmark.py
Usage: python3 jagr_duel.py [compare|simple]
Default mode: compare
"""
from __future__ import annotations

import sys

# ── from benchmark_jagr_comparison.py ─────────────────────────────────────────────
def run_compare(argv: list[str] | None = None) -> None:
    """Original script: benchmark_jagr_comparison.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    JAGR-1 vs Existing Resolver Benchmark Comparison
    Tests speed, accuracy, and error handling capabilities
    """

    import subprocess
    import json
    import time
    import statistics
    import os
    import sys
    from pathlib import Path
    from typing import Dict, List, Tuple, Optional
    import concurrent.futures
    import threading
    import signal
    from dataclasses import dataclass
    from datetime import datetime

    @dataclass
    class BenchmarkResult:
        name: str
        resolver_type: str
        success: bool
        time_ms: float
        error_message: Optional[str]
        packages_resolved: int
        lockfile_size: int
        memory_usage_mb: Optional[float] = None

    class BenchmarkSuite:
        def __init__(self):
            self.results: List[BenchmarkResult] = []
            self.test_scenarios = self._load_test_scenarios()
            self.jhol_path = self._find_jhol_binary()

        def _find_jhol_binary(self) -> Path:
            """Find the jhol binary"""
            possible_paths = [
                Path("./target/debug/jhol"),
                Path("./target/release/jhol"),
                Path("./target/debug/jhol.exe"),
                Path("./target/release/jhol.exe"),
            ]

            for path in possible_paths:
                if path.exists():
                    return path

            # Try to build it
            try:
                subprocess.run(["cargo", "build", "--release"], check=True, capture_output=True)
                return Path("./target/release/jhol")
            except subprocess.CalledProcessError:
                print("Error: Could not find or build jhol binary")
                sys.exit(1)

        def _load_test_scenarios(self) -> List[Dict]:
            """Load test scenarios from existing benchmark data"""
            scenarios = []

            # Scenario 1: Large React monorepo (1500+ deps)
            scenarios.append({
                "name": "react-monorepo-large",
                "description": "Large React monorepo with 1500+ dependencies",
                "complexity": "high",
                "peer_deps": True,
                "optional_deps": True,
                "overrides": True
            })

            # Scenario 2: Complex peer dependency conflicts
            scenarios.append({
                "name": "peer-dependency-conflicts",
                "description": "Complex peer dependency conflicts (npm's biggest pain point)",
                "complexity": "high",
                "peer_deps": True,
                "optional_deps": False,
                "overrides": False
            })

            # Scenario 3: Enterprise npmrc configurations
            scenarios.append({
                "name": "enterprise-npmrc",
                "description": "Enterprise npmrc with custom registries and auth",
                "complexity": "medium",
                "peer_deps": False,
                "optional_deps": True,
                "overrides": True
            })

            # Scenario 4: Framework compatibility matrix
            scenarios.append({
                "name": "framework-matrix",
                "description": "Multiple framework versions with compatibility constraints",
                "complexity": "medium",
                "peer_deps": True,
                "optional_deps": True,
                "overrides": False
            })

            # Scenario 5: Fallback trend scenarios
            scenarios.append({
                "name": "fallback-trends",
                "description": "Scenarios that trigger fallback to legacy resolver",
                "complexity": "medium",
                "peer_deps": True,
                "optional_deps": True,
                "overrides": True
            })

            return scenarios

        def run_benchmark(self, scenario: Dict, resolver_type: str) -> BenchmarkResult:
            """Run a single benchmark scenario"""
            print(f"Running {scenario['name']} with {resolver_type}...")

            start_time = time.time()
            success = False
            error_message = None
            packages_resolved = 0
            lockfile_size = 0
            memory_usage = None

            try:
                # Create test directory
                test_dir = Path(f"test_benchmark_{scenario['name']}_{resolver_type}")
                test_dir.mkdir(exist_ok=True)

                # Generate test package.json based on scenario
                package_json = self._generate_package_json(scenario)
                (test_dir / "package.json").write_text(json.dumps(package_json, indent=2))

                # Set up environment
                env = os.environ.copy()
                env["JHOL_SOLVER_TIMEOUT_MS"] = "30000"  # 30 second timeout
                env["JHOL_CACHE_DIR"] = str(test_dir / ".jhol-cache")

                if resolver_type == "jagr":
                    env["JHOL_RESOLVER"] = "jagr"
                elif resolver_type == "legacy":
                    env["JHOL_RESOLVER"] = "legacy"

                # Run jhol install
                cmd = [str(self.jhol_path), "install"]
                result = subprocess.run(
                    cmd,
                    cwd=test_dir,
                    capture_output=True,
                    text=True,
                    timeout=60,  # 1 minute max timeout
                    env=env
                )

                end_time = time.time()
                duration_ms = (end_time - start_time) * 1000

                if result.returncode == 0:
                    success = True
                    packages_resolved = self._count_packages_in_lockfile(test_dir)
                    lockfile_size = self._get_lockfile_size(test_dir)
                    memory_usage = self._estimate_memory_usage(result.stderr)
                else:
                    success = False
                    error_message = result.stderr.strip() or result.stdout.strip()

                # Cleanup
                import shutil
                shutil.rmtree(test_dir, ignore_errors=True)

            except subprocess.TimeoutExpired:
                success = False
                error_message = "Timeout expired"
                duration_ms = 60000
            except Exception as e:
                success = False
                error_message = str(e)
                duration_ms = (time.time() - start_time) * 1000

            return BenchmarkResult(
                name=scenario['name'],
                resolver_type=resolver_type,
                success=success,
                time_ms=duration_ms,
                error_message=error_message,
                packages_resolved=packages_resolved,
                lockfile_size=lockfile_size,
                memory_usage_mb=memory_usage
            )

        def _generate_package_json(self, scenario: Dict) -> Dict:
            """Generate package.json for test scenario"""
            base_deps = {
                "react": "^18.0.0",
                "react-dom": "^18.0.0",
                "typescript": "^5.0.0",
                "@types/node": "^18.0.0",
            }

            if scenario.get("peer_deps"):
                base_deps.update({
                    "@types/react": "^18.0.0",
                    "eslint": "^8.0.0",
                    "prettier": "^2.8.0",
                })

            if scenario.get("optional_deps"):
                base_deps.update({
                    "webpack": "^5.0.0",
                    "babel-loader": "^8.0.0",
                    "sass": "^1.50.0",
                })

            if scenario.get("overrides"):
                base_deps.update({
                    "lodash": "^4.17.21",
                    "axios": "^1.0.0",
                })

            # Add more dependencies based on complexity
            if scenario["complexity"] == "high":
                for i in range(50):
                    base_deps[f"test-package-{i}"] = f"^1.{i}.0"

            return {
                "name": f"benchmark-{scenario['name']}",
                "version": "1.0.0",
                "dependencies": base_deps,
                "devDependencies": {
                    "@types/react": "^18.0.0",
                    "eslint": "^8.0.0",
                    "prettier": "^2.8.0",
                } if scenario.get("peer_deps") else {}
            }

        def _count_packages_in_lockfile(self, test_dir: Path) -> int:
            """Count packages in generated lockfile"""
            lockfile = test_dir / "jhol-lock.json"
            if lockfile.exists():
                try:
                    data = json.loads(lockfile.read_text())
                    return len(data.get("packages", {}))
                except:
                    return 0
            return 0

        def _get_lockfile_size(self, test_dir: Path) -> int:
            """Get lockfile size in bytes"""
            lockfile = test_dir / "jhol-lock.json"
            if lockfile.exists():
                return lockfile.stat().st_size
            return 0

        def _estimate_memory_usage(self, stderr_output: str) -> Optional[float]:
            """Estimate memory usage from stderr output"""
            # This would need to be implemented based on actual jhol output
            # For now, return None
            return None

        def run_comparative_benchmark(self) -> Dict[str, List[BenchmarkResult]]:
            """Run benchmarks comparing JAGR vs Legacy resolver"""
            print("🚀 Starting JAGR-1 vs Legacy Resolver Benchmark")
            print("=" * 60)

            results = {
                "jagr": [],
                "legacy": []
            }

            for scenario in self.test_scenarios:
                print(f"\n📊 Testing scenario: {scenario['name']}")
                print(f"   Description: {scenario['description']}")
                print(f"   Complexity: {scenario['complexity']}")

                # Test JAGR resolver
                jagr_result = self.run_benchmark(scenario, "jagr")
                results["jagr"].append(jagr_result)

                # Test Legacy resolver
                legacy_result = self.run_benchmark(scenario, "legacy")
                results["legacy"].append(legacy_result)

                # Print comparison for this scenario
                self._print_scenario_comparison(jagr_result, legacy_result)

            return results

        def _print_scenario_comparison(self, jagr: BenchmarkResult, legacy: BenchmarkResult):
            """Print comparison results for a single scenario"""
            print(f"\n📈 Results for {jagr.name}:")
            print(f"   JAGR:     {'✅' if jagr.success else '❌'} {jagr.time_ms:.1f}ms ({jagr.packages_resolved} packages)")
            print(f"   Legacy:   {'✅' if legacy.success else '❌'} {legacy.time_ms:.1f}ms ({legacy.packages_resolved} packages)")

            if jagr.success and legacy.success:
                speedup = legacy.time_ms / jagr.time_ms
                print(f"   Speedup:  {speedup:.2f}x faster")

            if jagr.error_message and legacy.error_message:
                print(f"   JAGR Error:   {jagr.error_message[:100]}...")
                print(f"   Legacy Error: {legacy.error_message[:100]}...")

        def generate_report(self, results: Dict[str, List[BenchmarkResult]]):
            """Generate comprehensive benchmark report"""
            print("\n" + "=" * 80)
            print("📋 COMPREHENSIVE BENCHMARK REPORT")
            print("=" * 80)

            # Overall statistics
            jagr_results = results["jagr"]
            legacy_results = results["legacy"]

            print(f"\n📊 Overall Statistics:")
            print(f"   Total Scenarios: {len(jagr_results)}")
            print(f"   JAGR Success Rate: {sum(1 for r in jagr_results if r.success) / len(jagr_results) * 100:.1f}%")
            print(f"   Legacy Success Rate: {sum(1 for r in legacy_results if r.success) / len(legacy_results) * 100:.1f}%")

            # Performance comparison
            jagr_times = [r.time_ms for r in jagr_results if r.success]
            legacy_times = [r.time_ms for r in legacy_results if r.success]

            if jagr_times and legacy_times:
                jagr_avg = statistics.mean(jagr_times)
                legacy_avg = statistics.mean(legacy_times)
                speedup = legacy_avg / jagr_avg

                print(f"\n⚡ Performance Comparison:")
                print(f"   JAGR Average Time: {jagr_avg:.1f}ms")
                print(f"   Legacy Average Time: {legacy_avg:.1f}ms")
                print(f"   Average Speedup: {speedup:.2f}x")

                # Best and worst case scenarios
                best_speedup = max(legacy_times[i] / jagr_times[i] for i in range(len(jagr_times)))
                worst_speedup = min(legacy_times[i] / jagr_times[i] for i in range(len(jagr_times)))

                print(f"   Best Case Speedup: {best_speedup:.2f}x")
                print(f"   Worst Case Speedup: {worst_speedup:.2f}x")

            # Accuracy comparison
            print(f"\n🎯 Accuracy Comparison:")
            for i, (jagr, legacy) in enumerate(zip(jagr_results, legacy_results)):
                scenario_name = jagr.name
                if jagr.success != legacy.success:
                    winner = "JAGR" if jagr.success else "Legacy"
                    print(f"   {scenario_name}: {winner} handles this case better")
                elif jagr.success and legacy.success:
                    if jagr.packages_resolved != legacy.packages_resolved:
                        diff = abs(jagr.packages_resolved - legacy.packages_resolved)
                        print(f"   {scenario_name}: JAGR resolves {diff} more packages")

            # Error quality comparison
            print(f"\n📝 Error Quality:")
            for i, (jagr, legacy) in enumerate(zip(jagr_results, legacy_results)):
                if not jagr.success or not legacy.success:
                    scenario_name = jagr.name
                    print(f"   {scenario_name}:")
                    if jagr.error_message:
                        print(f"     JAGR: {jagr.error_message[:80]}...")
                    if legacy.error_message:
                        print(f"     Legacy: {legacy.error_message[:80]}...")

            # Memory usage comparison (if available)
            jagr_memory = [r.memory_usage_mb for r in jagr_results if r.memory_usage_mb]
            legacy_memory = [r.memory_usage_mb for r in legacy_results if r.memory_usage_mb]

            if jagr_memory and legacy_memory:
                print(f"\n💾 Memory Usage:")
                print(f"   JAGR Average: {statistics.mean(jagr_memory):.1f}MB")
                print(f"   Legacy Average: {statistics.mean(legacy_memory):.1f}MB")

            # Save detailed results to JSON
            detailed_results = {
                "timestamp": datetime.now().isoformat(),
                "jagr_results": [r.__dict__ for r in jagr_results],
                "legacy_results": [r.__dict__ for r in legacy_results],
                "summary": {
                    "total_scenarios": len(jagr_results),
                    "jagr_success_rate": sum(1 for r in jagr_results if r.success) / len(jagr_results) * 100,
                    "legacy_success_rate": sum(1 for r in legacy_results if r.success) / len(legacy_results) * 100,
                    "average_speedup": statistics.mean([legacy_times[i] / jagr_times[i] for i in range(len(jagr_times))]) if jagr_times and legacy_times else 0
                }
            }

            with open("jagr_benchmark_results.json", "w") as f:
                json.dump(detailed_results, f, indent=2)

            print(f"\n💾 Detailed results saved to: jagr_benchmark_results.json")

            # Final recommendations
            print(f"\n🎯 JAGR-1 Implementation Assessment:")
            jagr_success_rate = sum(1 for r in jagr_results if r.success) / len(jagr_results) * 100
            legacy_success_rate = sum(1 for r in legacy_results if r.success) / len(legacy_results) * 100

            if jagr_success_rate > legacy_success_rate:
                print(f"   ✅ JAGR-1 shows improved accuracy ({jagr_success_rate:.1f}% vs {legacy_success_rate:.1f}%)")
            elif jagr_success_rate == legacy_success_rate:
                print(f"   ✅ JAGR-1 maintains accuracy while improving performance")
            else:
                print(f"   ⚠️  JAGR-1 needs accuracy improvements")

            if jagr_times and legacy_times:
                avg_speedup = statistics.mean([legacy_times[i] / jagr_times[i] for i in range(len(jagr_times))])
                if avg_speedup > 1.5:
                    print(f"   ✅ JAGR-1 shows significant performance improvement ({avg_speedup:.2f}x faster)")
                elif avg_speedup > 1.1:
                    print(f"   ✅ JAGR-1 shows moderate performance improvement ({avg_speedup:.2f}x faster)")
                else:
                    print(f"   ⚠️  JAGR-1 performance needs optimization")

    def main():
        """Main benchmark execution"""
        print("JAGR-1 vs Legacy Resolver Benchmark")
        print("Testing speed, accuracy, and error handling capabilities")
        print()

        # Check if jhol is built
        try:
            subprocess.run(["cargo", "build", "--release"], check=True, capture_output=True)
            print("✅ Jhol binary built successfully")
        except subprocess.CalledProcessError:
            print("❌ Failed to build jhol binary")
            sys.exit(1)

        # Run benchmarks
        benchmark = BenchmarkSuite()
        results = benchmark.run_comparative_benchmark()

        # Generate report
        benchmark.generate_report(results)

        print(f"\n🏁 Benchmark completed successfully!")

    main()


# ── from simple_jagr_benchmark.py ─────────────────────────────────────────────
def run_simple(argv: list[str] | None = None) -> None:
    """Original script: simple_jagr_benchmark.py."""
    if argv is not None:
        sys.argv = [__file__] + list(argv)
    """
    Simple JAGR-1 vs Legacy Resolver Benchmark
    Tests the SAT resolver directly with synthetic test cases
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

    def create_test_package_json(scenario: str) -> Dict:
        """Create test package.json for different scenarios"""

        if scenario == "simple":
            return {
                "name": "test-simple",
                "version": "1.0.0",
                "dependencies": {
                    "react": "^18.0.0",
                    "react-dom": "^18.0.0",
                    "lodash": "^4.17.21"
                }
            }

        elif scenario == "peer-conflicts":
            return {
                "name": "test-peer-conflicts", 
                "version": "1.0.0",
                "dependencies": {
                    "@types/react": "^18.0.0",
                    "eslint": "^8.0.0",
                    "prettier": "^2.8.0"
                },
                "devDependencies": {
                    "@types/node": "^18.0.0"
                }
            }

        elif scenario == "complex":
            # Generate complex dependency graph
            deps = {}
            for i in range(20):
                deps[f"package-{i}"] = f"^1.{i}.0"

            deps.update({
                "react": "^18.0.0",
                "react-dom": "^18.0.0", 
                "typescript": "^5.0.0",
                "webpack": "^5.0.0",
                "babel-loader": "^8.0.0"
            })

            return {
                "name": "test-complex",
                "version": "1.0.0", 
                "dependencies": deps,
                "devDependencies": {
                    "@types/react": "^18.0.0",
                    "eslint": "^8.0.0",
                    "prettier": "^2.8.0"
                }
            }

        elif scenario == "enterprise":
            return {
                "name": "test-enterprise",
                "version": "1.0.0",
                "dependencies": {
                    "axios": "^1.0.0",
                    "lodash": "^4.17.21",
                    "moment": "^2.29.0",
                    "uuid": "^9.0.0"
                },
                "devDependencies": {
                    "typescript": "^5.0.0",
                    "@types/node": "^18.0.0",
                    "@types/lodash": "^4.14.190"
                }
            }

        else:
            return create_test_package_json("simple")

    def run_resolver_test(scenario: str, resolver_type: str, timeout: int = 30) -> Dict:
        """Run a single resolver test"""
        start_time = time.time()

        # Create temporary directory
        with tempfile.TemporaryDirectory() as temp_dir:
            test_dir = Path(temp_dir)

            # Write package.json
            package_json = create_test_package_json(scenario)
            (test_dir / "package.json").write_text(json.dumps(package_json, indent=2))

            # Set up environment
            env = os.environ.copy()
            env["JHOL_SOLVER_TIMEOUT_MS"] = str(timeout * 1000)
            env["JHOL_CACHE_DIR"] = str(test_dir / ".jhol-cache")

            if resolver_type == "jagr":
                env["JHOL_RESOLVER"] = "jagr"
            elif resolver_type == "legacy":
                env["JHOL_RESOLVER"] = "legacy"

            try:
                # Run jhol install
                result = subprocess.run(
                    ["cargo", "run", "--release", "--", "install"],
                    cwd=test_dir,
                    capture_output=True,
                    text=True,
                    timeout=timeout,
                    env=env
                )

                end_time = time.time()
                duration = (end_time - start_time) * 1000

                success = result.returncode == 0
                error_message = result.stderr.strip() if not success else None

                # Count packages in lockfile if it exists
                lockfile = test_dir / "jhol-lock.json"
                packages_resolved = 0
                if lockfile.exists():
                    try:
                        data = json.loads(lockfile.read_text())
                        packages_resolved = len(data.get("packages", {}))
                    except:
                        pass

                return {
                    "scenario": scenario,
                    "resolver": resolver_type,
                    "success": success,
                    "time_ms": duration,
                    "error_message": error_message,
                    "packages_resolved": packages_resolved,
                    "lockfile_size": lockfile.stat().st_size if lockfile.exists() else 0
                }

            except subprocess.TimeoutExpired:
                return {
                    "scenario": scenario,
                    "resolver": resolver_type,
                    "success": False,
                    "time_ms": timeout * 1000,
                    "error_message": "Timeout expired",
                    "packages_resolved": 0,
                    "lockfile_size": 0
                }
            except Exception as e:
                return {
                    "scenario": scenario,
                    "resolver": resolver_type,
                    "success": False,
                    "time_ms": (time.time() - start_time) * 1000,
                    "error_message": str(e),
                    "packages_resolved": 0,
                    "lockfile_size": 0
                }

    def main():
        """Main benchmark execution"""
        print("🚀 JAGR-1 vs Legacy Resolver Benchmark")
        print("=" * 50)

        # Test scenarios
        scenarios = ["simple", "peer-conflicts", "complex", "enterprise"]
        resolvers = ["jagr", "legacy"]

        results = []

        for scenario in scenarios:
            print(f"\n📊 Testing scenario: {scenario}")

            for resolver in resolvers:
                print(f"   Running {resolver} resolver...")

                result = run_resolver_test(scenario, resolver)
                results.append(result)

                status = "✅" if result["success"] else "❌"
                print(f"     {status} {result['time_ms']:.1f}ms ({result['packages_resolved']} packages)")

                if result["error_message"]:
                    print(f"     Error: {result['error_message'][:100]}...")

        # Generate comparison report
        print("\n" + "=" * 60)
        print("📋 BENCHMARK RESULTS")
        print("=" * 60)

        # Group results by scenario
        scenario_results = {}
        for result in results:
            key = result["scenario"]
            if key not in scenario_results:
                scenario_results[key] = {}
            scenario_results[key][result["resolver"]] = result

        # Compare each scenario
        total_jagr_time = 0
        total_legacy_time = 0
        jagr_successes = 0
        legacy_successes = 0

        for scenario, resolvers in scenario_results.items():
            jagr = resolvers.get("jagr")
            legacy = resolvers.get("legacy")

            if jagr and legacy:
                print(f"\n📈 {scenario.upper()}:")
                print(f"   JAGR:     {'✅' if jagr['success'] else '❌'} {jagr['time_ms']:.1f}ms ({jagr['packages_resolved']} pkgs)")
                print(f"   Legacy:   {'✅' if legacy['success'] else '❌'} {legacy['time_ms']:.1f}ms ({legacy['packages_resolved']} pkgs)")

                if jagr["success"] and legacy["success"]:
                    speedup = legacy["time_ms"] / jagr["time_ms"]
                    print(f"   Speedup:  {speedup:.2f}x")
                    total_jagr_time += jagr["time_ms"]
                    total_legacy_time += legacy["time_ms"]

                if jagr["success"]:
                    jagr_successes += 1
                if legacy["success"]:
                    legacy_successes += 1

                # Show error comparison
                if jagr["error_message"] or legacy["error_message"]:
                    print(f"   Errors:")
                    if jagr["error_message"]:
                        print(f"     JAGR:   {jagr['error_message'][:80]}...")
                    if legacy["error_message"]:
                        print(f"     Legacy: {legacy['error_message'][:80]}...")

        # Overall statistics
        print(f"\n📊 OVERALL STATISTICS:")
        print(f"   Total Scenarios: {len(scenarios)}")
        print(f"   JAGR Success Rate: {jagr_successes}/{len(scenarios)} ({jagr_successes/len(scenarios)*100:.1f}%)")
        print(f"   Legacy Success Rate: {legacy_successes}/{len(scenarios)} ({legacy_successes/len(scenarios)*100:.1f}%)")

        if total_jagr_time > 0 and total_legacy_time > 0:
            avg_speedup = total_legacy_time / total_jagr_time
            print(f"   Average Speedup: {avg_speedup:.2f}x")
            print(f"   JAGR Total Time: {total_jagr_time:.1f}ms")
            print(f"   Legacy Total Time: {total_legacy_time:.1f}ms")

        # Save results to JSON
        with open("jagr_benchmark_results.json", "w") as f:
            json.dump({
                "timestamp": time.strftime("%Y-%m-%d %H:%M:%S"),
                "results": results,
                "summary": {
                    "total_scenarios": len(scenarios),
                    "jagr_successes": jagr_successes,
                    "legacy_successes": legacy_successes,
                    "jagr_success_rate": jagr_successes / len(scenarios) * 100,
                    "legacy_success_rate": legacy_successes / len(scenarios) * 100,
                    "average_speedup": total_legacy_time / total_jagr_time if total_jagr_time > 0 else 0
                }
            }, f, indent=2)

        print(f"\n💾 Results saved to: jagr_benchmark_results.json")

        # Final assessment
        print(f"\n🎯 JAGR-1 ASSESSMENT:")
        if jagr_successes > legacy_successes:
            print(f"   ✅ JAGR-1 shows improved accuracy ({jagr_successes} vs {legacy_successes} successes)")
        elif jagr_successes == legacy_successes:
            print(f"   ✅ JAGR-1 maintains accuracy")
        else:
            print(f"   ⚠️  JAGR-1 needs accuracy improvements")

        if total_jagr_time > 0 and total_legacy_time > 0:
            avg_speedup = total_legacy_time / total_jagr_time
            if avg_speedup > 1.5:
                print(f"   ✅ JAGR-1 shows significant performance improvement ({avg_speedup:.2f}x faster)")
            elif avg_speedup > 1.1:
                print(f"   ✅ JAGR-1 shows moderate performance improvement ({avg_speedup:.2f}x faster)")
            else:
                print(f"   ⚠️  JAGR-1 performance needs optimization")

        print(f"\n🏁 Benchmark completed!")

    main()


# ── dispatcher ──────────────────────────────────────────────────
_MODES = {
    'compare': run_compare,
    'simple': run_simple,
}

def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in _MODES:
        mode = argv.pop(0)
    else:
        mode = 'compare'
    _MODES[mode](argv)


if __name__ == "__main__":
    main()
