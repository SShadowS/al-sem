"""TLS-09: per perf_bounds row, the median over 3 runs of each run's min, per profile;
the tightest bound; headroom (bound / median min); release-fast over release."""
import re, statistics, sys
S = sys.argv[1]
UNIT = {"ns": 1e-6, "µs": 1e-3, "ms": 1.0, "s": 1000.0}

def ms(tok):
    m = re.fullmatch(r"([0-9.]+)(ns|µs|ms|s)", tok)
    return float(m.group(1)) * UNIT[m.group(2)]

def rows(path):
    out = {}
    for line in open(path, encoding="utf-8"):
        if not line.startswith("[perf_bounds]"):
            continue
        name, rest = line[len("[perf_bounds] "):].split(": ", 1)
        if name == "compute_all scaling":
            r = float(re.search(r"ratio=([0-9.]+)", rest).group(1))
            out[name] = (r, 10.0, "ratio")  # COMPUTE_ALL_SCALING_FACTOR
            continue
        mm = re.search(r"min=(\S+)", rest)
        val = ms(mm.group(1)) if mm else ms(rest.strip())
        bounds = [ms(b) for b in re.findall(r"bound=(\S+)", rest)]
        out[name] = (val, min(bounds) if bounds else None, "ms")
    return out

data = {p: [rows(f"{S}/perfb-{p}-{i}.log") for i in (1, 2, 3)] for p in ("release", "release-fast")}
print("| Row | release (median of 3 mins) | release-fast | fast / release | Bound | Headroom release | Headroom release-fast |")
print("|---|---:|---:|---:|---:|---:|---:|")
for name in data["release"][0]:
    r = statistics.median(run[name][0] for run in data["release"])
    f = statistics.median(run[name][0] for run in data["release-fast"])
    b, unit = data["release"][0][name][1], data["release"][0][name][2]
    fmt = (lambda v: f"{v:.3f}") if unit == "ratio" else (lambda v: f"{v:.3f} ms")
    hr = f"{b / r:.1f}x" if b else "-"
    hf = f"{b / f:.1f}x" if b else "-"
    print(f"| {name} | {fmt(r)} | {fmt(f)} | {f / r:.2f} | {fmt(b) if b else 'none'} | {hr} | {hf} |")
