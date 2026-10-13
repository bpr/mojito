# A trait's abstract `Self` might be register-passable, so a requirement
# whose result names a read receiver's origin is rejected; `ref self` is the
# spelling that has memory.
# expect: cannot return 'self's origin, because it might expand to a RegisterPassable type
trait Viewable:
    def view(self) -> Span[Int, origin_of(self)]:
        ...


def main():
    print(1)
