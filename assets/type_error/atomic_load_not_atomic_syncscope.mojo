# expect: cannot specify syncscope without an atomic load
# `Atomic.load` always names its scope, which a non-atomic load cannot
# take; the pin's verifier rejects the instantiation.
from std.atomic import Atomic, Ordering


def main():
    var a = Atomic[Int](1)
    print(a.load[ordering=Ordering.NOT_ATOMIC]())
