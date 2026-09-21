# Compatibility wrapper for the earlier Mojito prototype.  New tuple displays
# resolve to `std.builtin.tuple.Tuple`; explicit PackTuple imports continue
# to work during the migration.

from std.builtin.tuple import Tuple

struct PackTuple[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    # The one compile-time-index hook, spelled as `Tuple`'s own is: an element
    # is read through a reference, because the element type is the pack's and
    # carries no `ImplicitlyCopyable` bound, so returning it by value would
    # demand a copy the element does not license.
    # The element is copied explicitly: the pack's bound is `Copyable`, not
    # `ImplicitlyCopyable`, so reading one out of a borrowed receiver needs the
    # `.copy()` both compilers name. Returning a value rather than a reference
    # also keeps `t[i] = v` rejected, as it is on `Tuple`.
    def __getitem__[index: Int](self) -> Self.Ts[index]:
        return self.storage[index].copy()

    def __getitem_param__[index: Int](self) -> Self.Ts[index]:
        return self.storage[index].copy()

    def __len__(self) -> Int:
        return len(self.storage)
