# Upstream's `std/hash/hasher.mojo`: the `Hasher` trait and the default
# hasher aliases. The prelude does not export `Hasher`, as upstream's does
# not.
#
# The requirement set is Mojito's (R25): `__init__`, `_update_with_bytes`,
# and `update` over a `Hashable` value stand where upstream requires
# `update(mut self, value: ImmSpan[Byte, _])` and a `Defaultable` bound.
# `_update_with_simd` infers its vector's dtype and width per call, as
# upstream's `SIMD[_, _]` does: one template serves every hashed scalar and
# vector type, and every scalar leaf reaches it as itself with `-0.0` folded
# first, as upstream's `SIMD.__hash__`.

from ._ahash import AHasher
from ._fnv1a import Fnv1a
from .hash import Hashable

comptime default_hasher = AHasher[SIMD[DType.uint64, 4](0)]
comptime default_comp_time_hasher = Fnv1a


trait Hasher:
    def __init__(out self):
        ...

    def _update_with_bytes(mut self, data: Span[Byte, _]):
        ...

    def _update_with_simd(mut self, value: SIMD[_, _]):
        ...

    def update(mut self, value: Some[Hashable]):
        ...

    def finish(var self) -> UInt64:
        ...
