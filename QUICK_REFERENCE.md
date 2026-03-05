# Quick Reference

## Commands

```bash
# Basic run
msi-calc -r /data/GRCh38.fa -p sample_pairs.csv

# Custom output directory
msi-calc -r /data/GRCh38.fa -p pairs.csv -o /data/results

# Tune parallelism (8 jobs x 4 threads = 32 cores)
msi-calc -r /data/GRCh38.fa -p pairs.csv -j 8 -t 4

# Resume interrupted run (same command)
msi-calc -r /data/GRCh38.fa -p pairs.csv -o /data/results

# Print version
msi-calc -V

# Print help
msi-calc -h
```

## Options

| Flag | Description | Default |
|------|-------------|---------|
| `-r` | Reference FASTA (required) | — |
| `-p` | Sample pairs CSV | `sample_pairs.csv` |
| `-o` | Output directory | `msi-results` |
| `-j` | Parallel jobs | Auto (CPU/4) |
| `-t` | Threads per job | `2` |
| `-V` | Print version | — |
| `-h` | Print help | — |

## Input CSV Format

```
tumor.bam,normal.bam    # paired
tumor2.bam,             # tumor-only
```

## Config File

`~/.config/msi-calc/config.json`:
```json
{
  "reference": "/path/to/ref.fa",
  "pairs": "/path/to/pairs.csv"
}
```

## Docker

```bash
docker build -t msi-calc .
docker run --rm -it \
  -v /ref:/data/ref:ro \
  -v /bams:/data/bams:ro \
  -v /out:/data/output \
  msi-calc -r /data/ref/ref.fa -p /data/bams/pairs.csv -o /data/output
```

## Key Behaviors

- **Resume**: Re-run the same command to continue after interruption
- **Scan cache**: microsatellites.list is generated once and reused
- **Ctrl+C**: Graceful shutdown, partial outputs cleaned up
- **Total CPU** = jobs x threads
