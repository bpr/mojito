# The `std.memory` package: upstream's import surface for allocation
# (`Layout`, `Allocation`, `ThinAllocation`, `alloc`, `dealloc`), inline
# uninitialized storage (`MaybeUninit`), the owning smart pointer
# (`OwnedPointer`), `forget_deinit` (`std.memory.memory`), and the bit
# reinterpretations `bitcast` and `pack_bits` (`std.memory.unsafe`), a
# pointer's `AddressSpace` (`std.memory.address_space`), and the `Pointer`
# aliases (`std.memory.pointer`). `Pointer` itself is not re-exported until
# the module declares it (R2414).
# `unsafe_alloc` is deliberately not re-exported: as upstream, it imports
# only from `std.memory.alloc`.

from .address_space import AddressSpace
from .alloc import Allocation, ThinAllocation, alloc, dealloc, Layout
from .maybe_uninit import MaybeUninit
from .owned_pointer import OwnedPointer
from .pointer import (
    ImmOpaquePointer,
    ImmPointer,
    MutOpaquePointer,
    MutPointer,
    OpaquePointer,
    OptionalImmPointer,
    OptionalMutPointer,
    OptionalPointer,
)
from .memory import forget_deinit
from .unsafe import bitcast, pack_bits
