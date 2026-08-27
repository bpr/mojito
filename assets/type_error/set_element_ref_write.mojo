# expect: must be mutable
# Set element yields are declaration-level immutable: writing through an
# element reference would corrupt the hash and uniqueness invariants
# (upstream accepts the write and then reports stale membership — a
# recorded strict-subset gap, see conformance set-ref-write-gap).
from std.collections.set import Set

def main():
    var s: Set[Int] = {1, 2}
    for ref x in s:
        x += 10
