# `Hashable` is a compiler-known trait whose single requirement is
#
#   def __hash__(self, mut hasher: Some[Hasher])
#
# (also accepted as `def __hash__[H: Hasher](self, mut hasher: H)`). A
# conformer that omits it receives the reflective default: every field is
# fed to `hasher.update` in declaration order, and every field must be
# Hashable. This docstring-only home lets `from std.hashlib.hash import
# Hashable` resolve.

from .hasher import Hasher, default_hasher


def hash[T: Hashable, //, HasherType: Hasher = default_hasher](
    hashable: T
) -> UInt64:
    var hasher = HasherType()
    hasher.update(hashable)
    return hasher^.finish()
