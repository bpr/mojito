# expect: does not conform to trait 'Hashable'
# List's Hashable conformance is conditional on its element: an element type
# without `__hash__` cannot satisfy `hash`'s bound.
@fieldwise_init
struct Foo(Copyable, Movable):
    var value: Int

def main():
    print(hash(List[Foo]()))
