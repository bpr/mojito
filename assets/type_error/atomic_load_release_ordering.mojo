# expect: invalid atomic ordering 'release' for load operation
# An atomic load cannot release; the pin's verifier rejects the
# instantiation that closes the ordering.
from std.atomic import Atomic, Ordering


def main():
    var a = Atomic[Int](10)
    print(a.load[ordering=Ordering.RELEASE]())
