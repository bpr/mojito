# Strict-subset gap: upstream accepts `for ref` element write-through on a
# Set (its hash-backed storage then reports stale membership — writing 1,2
# up to 11,12 makes `11 in s` print False upstream), while Mojito's set
# iterator yields declaration-level immutable element references and
# rejects the write outright ("must be mutable") to keep the hash and
# uniqueness invariants observable.
from std.collections.set import Set

def main():
    var s: Set[Int] = {1, 2}
    for ref x in s:
        x += 10
    print(11 in s, 12 in s)
