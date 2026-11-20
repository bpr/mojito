# The `std.memory` package: upstream's import surface for allocation
# (`Layout`, `Allocation`, `ThinAllocation`, `alloc`, `dealloc`), inline
# uninitialized storage (`MaybeUninit`), the owning smart pointer
# (`OwnedPointer`), `forget_deinit` (`std.memory.memory`), and the bit
# reinterpretations `bitcast` and `pack_bits` (`std.memory.unsafe`), a
# pointer's `AddressSpace` (`std.memory.address_space`), and `Pointer` and
# its aliases (`std.memory.pointer`).
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
    Pointer,
)
from .memory import forget_deinit
from .unsafe import bitcast, pack_bits
