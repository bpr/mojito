# Stack sampler body for scripts/sample-stacks: the wrapper script sends
# SIGUSR1 to the inferior; gdb stops on each one, we record the innermost
# frames, and continue. Histograms print at exit.
import gdb, collections, os
DEPTH = int(os.environ.get("PMP_DEPTH", "16"))
gdb.execute("set pagination off")
gdb.execute("set confirm off")
gdb.execute("set print frame-arguments none")
gdb.execute("set print address off")
gdb.execute("handle SIGUSR1 stop print nopass")
stacks = collections.Counter()
leaves = collections.Counter()
incl = collections.Counter()
n = 0
try:
    gdb.execute("run", to_string=True)
    while True:
        inf = gdb.selected_inferior()
        if not inf.is_valid() or inf.pid == 0:
            break
        try:
            frame = gdb.newest_frame()
        except gdb.error:
            break
        names = []
        while frame is not None and len(names) < DEPTH:
            names.append(frame.name() or "??")
            frame = frame.older()
        if names:
            n += 1
            leaves[names[0]] += 1
            for name in set(names):
                incl[name] += 1
            stacks[" <- ".join(names)] += 1
        try:
            gdb.execute("continue", to_string=True)
        except gdb.error:
            break
except gdb.error as e:
    print("gdb error:", e)
def short(name):
    return name if len(name) < 110 else name[:107] + "..."
print(f"\n=== {n} samples ===")
print("\n--- top self frames ---")
for name, c in leaves.most_common(30):
    print(f"{c:5d} {100.0*c/max(n,1):5.1f}%  {short(name)}")
print(f"\n--- top inclusive frames (innermost {DEPTH}) ---")
for name, c in incl.most_common(45):
    print(f"{c:5d} {100.0*c/max(n,1):5.1f}%  {short(name)}")
print("\n--- top stacks ---")
for s, c in stacks.most_common(10):
    print(f"{c:5d}  {s}\n")
