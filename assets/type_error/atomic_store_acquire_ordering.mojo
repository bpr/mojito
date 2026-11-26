# expect: invalid atomic ordering 'acquire' for store operation
# An atomic store cannot acquire; the pin's verifier rejects the
# instantiation that closes the ordering.
from std.atomic import Atomic, Ordering


def main():
    var a = Atomic[Int](1)
    a.store[ordering=Ordering.ACQUIRE](2)
    print(a.load())
