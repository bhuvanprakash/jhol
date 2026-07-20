# Production Checklist

This guide covers operational best practices for running Jhol in production environments.

## Pre-Deployment Checklist

- [ ] Run `jhol audit` and address all vulnerabilities
- [ ] Use `--frozen` or `jhol ci` in CI for deterministic builds
- [ ] Set appropriate timeouts for your network (`JHOL_RESOLVE_TIMEOUT_SECS`)
- [ ] Set up cache pruning schedule for large deployments
- [ ] Test your specific dependency graph with `jhol doctor`

## Timeout Recommendations

For production environments, configure network timeouts based on your infrastructure:

| Environment Variable | Description | Recommended Value |
|---|---|---|
| `JHOL_RESOLVE_TIMEOUT_SECS` | Timeout for registry metadata requests | 30-60s (internal registries), 60-120s (external) |
| `JHOL_CONNECT_TIMEOUT_SECS` | Connection establishment timeout | 10-15s |
| `JHOL_TARBALL_TIMEOUT_SECS` | Timeout for downloading tarballs | 120-300s for large packages |

### Setting Timeouts

```sh
export JHOL_RESOLVE_TIMEOUT_SECS=60
export JHOL_CONNECT_TIMEOUT_SECS=15
export JHOL_TARBALL_TIMEOUT_SECS=180
```

For CI environments with reliable networks, use tighter timeouts to fail fast:

```sh
export JHOL_RESOLVE_TIMEOUT_SECS=30
export JHOL_TARBALL_TIMEOUT_SECS=120
```

## Cache Management

Jhol's cache-first architecture significantly reduces network usage, but production deployments should manage cache size.

### Cache Commands

```sh
# View cache size
jhol cache size

# List cached packages
jhol cache list

# Prune old/unused cache entries
jhol cache prune

# Export cache for backup or transfer
jhol cache export /path/to/cache-backup.tar

# Import cache from backup
jhol cache import /path/to/cache-backup.tar

# Clean entire cache
jhol cache clean

# Get cache key for CI caching
jhol cache key
```

### Cache Pruning Schedule

For large deployments, schedule regular cache pruning:

```sh
# Weekly pruning (cron)
0 0 * * 0 jhol cache prune

# Monthly aggressive prune
0 0 1 * * jhol cache clean && jhol install
```

### CI Cache Strategy

Use `jhol cache key` to generate deterministic cache keys:

```yaml
# .github/workflows/example.yml
- name: Cache Jhol cache
  uses: actions/cache@v4
  with:
    path: ~/.cache/jhol
    key: jhol-cache-${{ hashFiles('package-lock.json') }}
```

## Security Checklist

- [ ] Run `jhol audit` before every deployment
- [ ] Use `jhol audit --gate` in CI to fail on vulnerabilities
- [ ] Generate SBOM regularly: `jhol sbom -o sbom.json`
- [ ] Review dependency tree with `jhol doctor`
- [ ] Use `--frozen` to prevent unexpected updates

### Security Commands

```sh
# Audit for vulnerabilities
jhol audit

# Attempt automatic fixes
jhol audit --fix

# Fail CI on vulnerabilities
jhol audit --gate

# Generate Software Bill of Materials
jhol sbom -o sbom.json

# Check dependency health
jhol doctor
```

## CI Best Practices

### Deterministic Installs

Always use frozen mode in CI to ensure reproducible builds:

```sh
# Using --frozen
jhol install --frozen

# Or use the dedicated CI command
jhol ci
```

### CI Configuration Examples

#### GitHub Actions

```yaml
- name: Install dependencies
  run: jhol ci
  env:
    JHOL_RESOLVE_TIMEOUT_SECS: 30
    JHOL_OFFLINE: ${{ vars.JHOL_OFFLINE }}
```

#### GitLab CI

```yaml
install:
  script:
    - jhol ci
  variables:
    JHOL_RESOLVE_TIMEOUT_SECS: "30"
```

#### CircleCI

```yaml
- run:
    name: Install dependencies
    command: jhol ci
    environment:
      JHOL_RESOLVE_TIMEOUT_SECS: 30
```

### Recommended CI Environment Variables

```sh
# Fail fast on network issues
JHOL_RESOLVE_TIMEOUT_SECS=30

# Deterministic mode
JHOL_FROZEN=1

# Optional: reduce noise
JHOL_LOG=quiet
```

## Workspace Management

For monorepos with multiple workspaces:

```sh
# Run install across all workspaces
jhol install --all-workspaces

# Audit all workspaces
jhol audit --all-workspaces

# Doctor all workspaces
jhol doctor --all-workspaces
```

## Monitoring

Track these metrics in production:

- Cache hit rate
- Install duration
- Network failure rate
- Audit findings over time

## Troubleshooting

### Network Issues

If experiencing timeouts:

1. Increase timeout values
2. Check registry availability
3. Use `--offline` if cache is populated

### Cache Issues

If installs fail unexpectedly:

```sh
jhol cache clean
jhol install
```

### Dependency Resolution

If encountering resolution issues:

```sh
jhol doctor
jhol doctor --fix
```

## Reference

- CLI Reference: See README.md command reference
- Configuration: See `.jholrc` configuration options
- Support: Open GitHub issues for problems
