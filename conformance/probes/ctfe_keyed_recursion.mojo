# Probe: may a compile-time evaluation call a value-keyed generic `def` that
# recurses under a `comptime if`?
#
# The pin (2026-10-02) prints `3`. Mojito stops with "VM CTFE failed for
# 'rep': unsupported feature: vm: unknown compile-time function 'rep'": the
# evaluation's subprogram excludes every compile-time-keyed `def`.
# docs/roadmap.md 3.116.
def rep[n: Int]() -> Int:
    comptime if n == 0:
        return 0
    else:
        return 1 + rep[n - 1]()

comptime R = rep[3]()

def main():
    print(R)
