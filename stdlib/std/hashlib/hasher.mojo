# `Hasher` is a compiler-known trait with current Mojo's requirement set:
#
#   def __init__(out self)
#   def _update_with_bytes(mut self, data: Span[Byte, _])
#   def _update_with_simd(mut self, value: UInt64)
#   def update(mut self, value: Some[Hashable])
#   def finish(var self) -> UInt64
#
# Mojito narrows `_update_with_simd` to one normalized `UInt64` leaf: the
# compiler zero-extends every scalar's bit pattern (folding `-0.0`) before
# the call, which is what both bundled hashers mix per lane anyway. This
# docstring-only home lets `from std.hashlib.hasher import Hasher` resolve.

from ._ahash import AHasher
from ._fnv1a import Fnv1a

comptime default_hasher = AHasher
comptime default_comp_time_hasher = Fnv1a
