# expect: unhandled error: __module$std$iter$StopIteration()
# A direct `__next__` call past exhaustion propagates the nullary
# StopIteration struct itself; the native message must spell the VM's
# display of that value byte-for-byte.
from std.iter import StopIteration

def main() raises StopIteration:
    var source = range(2)
    var it = source.__iter__()
    print(it.__next__())
    print(it.__next__())
    print(it.__next__())
