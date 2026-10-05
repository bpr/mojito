# `Hasher` is a compiler-known trait with current Mojo's requirement set:
#
#   def __init__(out self)
#   def _update_with_bytes(mut self, data: Span[Byte, _])
#   def _update_with_simd(mut self, value: SIMD[_, _])
#   def update(mut self, value: Some[Hashable])
#   def finish(var self) -> UInt64
#
# `_update_with_simd` infers its vector's dtype and width per call, as
# upstream's `SIMD[_, _]` does: one template serves every hashed scalar and
# vector type, and every scalar leaf reaches it as itself with `-0.0` folded
# first, as upstream's `SIMD.__hash__`. This docstring-only home lets
# `from std.hashlib.hasher import Hasher` resolve.

from ._ahash import AHasher
from ._fnv1a import Fnv1a

comptime default_hasher = AHasher[SIMD[DType.uint64, 4](0)]
comptime default_comp_time_hasher = Fnv1a
