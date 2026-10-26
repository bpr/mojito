# Upstream's `std/hash/hash.mojo`: the `Hashable` trait and `hash()`. The
# prelude exports `Hashable`, as upstream's does.
#
# A conformer that omits `__hash__` receives the reflective default: every
# field is fed to `hasher.update` in declaration order, and every field must
# be Hashable. The compiler synthesizes that body until the trait's own can
# read `reflect[Self]` (R269).

from .hasher import Hasher, default_hasher


trait Hashable:
    def __hash__(self, mut hasher: Some[Hasher]):
        ...


def hash[T: Hashable, //, HasherType: Hasher = default_hasher](
    hashable: T
) -> UInt64:
    var hasher = HasherType()
    hasher.update(hashable)
    return hasher^.finish()


# Upstream's raw-bytes overload: hash `n` bytes behind `bytes` under
# `HasherType` (`Span(unsafe_ptr=, length=)` over the untracked view).
def hash[HasherType: Hasher = default_hasher](
    bytes: ImmPointer[UInt8, _], n: Int
) -> UInt64:
    var hasher = HasherType()
    hasher._update_with_bytes(Span[Byte](unsafe_ptr=bytes, length=n))
    return hasher^.finish()
