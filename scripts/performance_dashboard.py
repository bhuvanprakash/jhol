#!/usr/bin/env python3
"""
Jhol Performance Dashboard Generator

Generates HTML dashboard from benchmark results for performance monitoring
and trend analysis.

Usage:
    python3 scripts/performance_dashboard.py --input benchmark-results.json --output dashboard.html
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path
from typing import Dict, List, Any, Optional


def load_benchmark_results(path: str) -> Dict[str, Any]:
    """Load benchmark results from JSON file."""
    with open(path, 'r') as f:
        return json.load(f)


def calculate_statistics(results: List[Dict[str, Any]]) -> Dict[str, float]:
    """Calculate statistics from benchmark results."""
    if not results:
        return {}
    
    times = [r.get('duration_ms', 0) for r in results]
    return {
        'min': min(times),
        'max': max(times),
        'avg': sum(times) / len(times),
        'median': sorted(times)[len(times) // 2],
        'stddev': calculate_stddev(times),
    }


def calculate_stddev(values: List[float]) -> float:
    """Calculate standard deviation."""
    if len(values) < 2:
        return 0.0
    
    avg = sum(values) / len(values)
    variance = sum((x - avg) ** 2 for x in values) / len(values)
    return variance ** 0.5


def generate_trend_chart_data(results: List[Dict[str, Any]]) -> str:
    """Generate Chart.js data for trend chart."""
    labels = []
    data_points = []
    
    for i, result in enumerate(results):
        timestamp = result.get('timestamp', f'Run {i+1}')
        duration = result.get('duration_ms', 0)
        labels.append(timestamp)
        data_points.append(duration)
    
    return f"""
    {{
        labels: {json.dumps(labels)},
        datasets: [{{
            label: 'Install Duration (ms)',
            data: {json.dumps(data_points)},
            borderColor: 'rgb(75, 192, 192)',
            tension: 0.1,
            fill: true,
            backgroundColor: 'rgba(75, 192, 192, 0.2)'
        }}]
    }}
    """


def generate_comparison_table(current: Dict[str, Any], baseline: Optional[Dict[str, Any]]) -> str:
    """Generate HTML table comparing current vs baseline."""
    if not baseline:
        return "<p>No baseline data available for comparison.</p>"
    
    rows = []
    metrics = ['cold_install_ms', 'warm_install_ms', 'resolution_time_ms', 'cache_lookup_ms']
    
    for metric in metrics:
        current_val = current.get(metric, 0)
        baseline_val = baseline.get(metric, 0)
        
        if baseline_val > 0:
            change = ((current_val - baseline_val) / baseline_val) * 100
            change_class = 'improvement' if change < 0 else 'regression'
            change_str = f"{change:+.1f}%"
        else:
            change_class = ''
            change_str = 'N/A'
        
        rows.append(f"""
        <tr>
            <td>{metric.replace('_', ' ').title()}</td>
            <td>{current_val:.0f} ms</td>
            <td>{baseline_val:.0f} ms</td>
            <td class="{change_class}">{change_str}</td>
        </tr>
        """)
    
    return f"""
    <table>
        <thead>
            <tr>
                <th>Metric</th>
                <th>Current</th>
                <th>Baseline</th>
                <th>Change</th>
            </tr>
        </thead>
        <tbody>
            {''.join(rows)}
        </tbody>
    </table>
    """


def generate_dashboard_html(
    results: Dict[str, Any],
    baseline: Optional[Dict[str, Any]] = None,
    output_path: str = 'dashboard.html'
) -> None:
    """Generate complete HTML dashboard."""
    
    timestamp = datetime.now().strftime('%Y-%m-%d %H:%M:%S')
    version = results.get('version', 'unknown')
    
    # Extract scenario results
    scenarios = results.get('scenarios', [])
    overall_stats = results.get('overall_stats', {})
    
    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Jhol Performance Dashboard</title>
    <script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
    <style>
        :root {{
            --primary: #2563eb;
            --success: #16a34a;
            --warning: #ca8a04;
            --danger: #dc2626;
            --bg: #f8fafc;
            --card-bg: #ffffff;
            --text: #1e293b;
            --text-muted: #64748b;
        }}
        
        * {{
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }}
        
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            background: var(--bg);
            color: var(--text);
            line-height: 1.6;
            padding: 2rem;
        }}
        
        .container {{
            max-width: 1400px;
            margin: 0 auto;
        }}
        
        header {{
            margin-bottom: 2rem;
        }}
        
        h1 {{
            font-size: 2rem;
            font-weight: 700;
            color: var(--primary);
            margin-bottom: 0.5rem;
        }}
        
        .subtitle {{
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
        
        .grid {{
            display: grid;
            grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
            gap: 1.5rem;
            margin-bottom: 2rem;
        }}
        
        .card {{
            background: var(--card-bg);
            border-radius: 0.5rem;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        
        .card h2 {{
            font-size: 1.25rem;
            margin-bottom: 1rem;
            color: var(--text);
        }}
        
        .stat {{
            display: flex;
            justify-content: space-between;
            padding: 0.75rem 0;
            border-bottom: 1px solid #e2e8f0;
        }}
        
        .stat:last-child {{
            border-bottom: none;
        }}
        
        .stat-label {{
            color: var(--text-muted);
        }}
        
        .stat-value {{
            font-weight: 600;
        }}
        
        .improvement {{
            color: var(--success);
        }}
        
        .regression {{
            color: var(--danger);
        }}
        
        table {{
            width: 100%;
            border-collapse: collapse;
            margin-top: 1rem;
        }}
        
        th, td {{
            padding: 0.75rem;
            text-align: left;
            border-bottom: 1px solid #e2e8f0;
        }}
        
        th {{
            background: #f1f5f9;
            font-weight: 600;
        }}
        
        .chart-container {{
            position: relative;
            height: 400px;
            margin-top: 1rem;
        }}
        
        .summary-grid {{
            display: grid;
            grid-template-columns: repeat(4, 1fr);
            gap: 1rem;
            margin-bottom: 2rem;
        }}
        
        .summary-card {{
            background: var(--card-bg);
            padding: 1.5rem;
            border-radius: 0.5rem;
            text-align: center;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }}
        
        .summary-value {{
            font-size: 2rem;
            font-weight: 700;
            color: var(--primary);
        }}
        
        .summary-label {{
            color: var(--text-muted);
            font-size: 0.875rem;
            margin-top: 0.5rem;
        }}
        
        footer {{
            margin-top: 3rem;
            text-align: center;
            color: var(--text-muted);
            font-size: 0.875rem;
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>🚀 Jhol Performance Dashboard</h1>
            <p class="subtitle">Generated: {timestamp} | Version: {version}</p>
        </header>
        
        <div class="summary-grid">
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('total_scenarios', len(scenarios))}</div>
                <div class="summary-label">Total Scenarios</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('jagr_success_rate', 'N/A')}</div>
                <div class="summary-label">JAGR Success Rate</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('average_speedup', 'N/A')}</div>
                <div class="summary-label">Average Speedup</div>
            </div>
            <div class="summary-card">
                <div class="summary-value">{overall_stats.get('total_time_jagr_ms', 'N/A')} ms</div>
                <div class="summary-label">Total JAGR Time</div>
            </div>
        </div>
        
        <div class="grid">
            <div class="card">
                <h2>📊 Performance Trends</h2>
                <div class="chart-container">
                    <canvas id="trendChart"></canvas>
                </div>
            </div>
            
            <div class="card">
                <h2>📈 Scenario Comparison</h2>
                {generate_comparison_table(results, baseline)}
            </div>
        </div>
        
        <div class="card">
            <h2>📋 Detailed Results</h2>
            <table>
                <thead>
                    <tr>
                        <th>Scenario</th>
                        <th>Status</th>
                        <th>JAGR Time (ms)</th>
                        <th>Legacy Time (ms)</th>
                        <th>Speedup</th>
                    </tr>
                </thead>
                <tbody>
                    {generate_scenario_rows(scenarios)}
                </tbody>
            </table>
        </div>
        
        <footer>
            <p>Jhol Performance Dashboard | Built with ❤️ for better package management</p>
        </footer>
    </div>
    
    <script>
        // Trend Chart
        const trendCtx = document.getElementById('trendChart').getContext('2d');
        new Chart(trendCtx, {{
            type: 'line',
            data: {generate_trend_chart_data(scenarios)},
            options: {{
                responsive: true,
                maintainAspectRatio: false,
                plugins: {{
                    legend: {{
                        display: true,
                        position: 'top'
                    }}
                }},
                scales: {{
                    y: {{
                        beginAtZero: true,
                        title: {{
                            display: true,
                            text: 'Duration (ms)'
                        }}
                    }},
                    x: {{
                        title: {{
                            display: true,
                            text: 'Test Run'
                        }}
                    }}
                }}
            }}
        }});
    </script>
</body>
</html>
"""
    
    with open(output_path, 'w') as f:
        f.write(html)
    
    print(f"Dashboard generated: {output_path}")


def generate_scenario_rows(scenarios: List[Dict[str, Any]]) -> str:
    """Generate HTML table rows for scenarios."""
    rows = []
    
    for scenario in scenarios:
        name = scenario.get('name', 'Unknown')
        status = scenario.get('status', 'unknown')
        jagr_time = scenario.get('jagr_time_ms', 0)
        legacy_time = scenario.get('legacy_time_ms', 0)
        
        if legacy_time > 0 and jagr_time > 0:
            speedup = legacy_time / jagr_time
            speedup_str = f"{speedup:.2f}x"
        else:
            speedup_str = 'N/A'
        
        status_class = 'improvement' if status == 'success' else 'regression'
        
        rows.append(f"""
        <tr>
            <td>{name}</td>
            <td class="{status_class}">{status}</td>
            <td>{jagr_time:.0f}</td>
            <td>{legacy_time:.0f}</td>
            <td>{speedup_str}</td>
        </tr>
        """)
    
    return ''.join(rows)


def main():
    parser = argparse.ArgumentParser(description='Generate Jhol Performance Dashboard')
    parser.add_argument('--input', '-i', required=True, help='Input benchmark results JSON')
    parser.add_argument('--baseline', '-b', help='Baseline results for comparison')
    parser.add_argument('--output', '-o', default='dashboard.html', help='Output HTML file')
    
    args = parser.parse_args()
    
    results = load_benchmark_results(args.input)
    baseline = None
    
    if args.baseline:
        try:
            baseline = load_benchmark_results(args.baseline)
        except FileNotFoundError:
            print(f"Warning: Baseline file not found: {args.baseline}")
    
    generate_dashboard_html(results, baseline, args.output)


if __name__ == '__main__':
    main()
