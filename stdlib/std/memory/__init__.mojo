# The `std.memory` package: upstream's import surface for allocation
# (`Layout`, `Allocation`, `ThinAllocation`, `alloc`, `dealloc`), inline
# uninitialized storage (`MaybeUninit`), and the owning smart pointer
# (`OwnedPointer`). `unsafe_alloc` is deliberately not re-exported: as
# upstream, it imports only from `std.memory.alloc`.

from .alloc import Allocation, ThinAllocation, alloc, dealloc, Layout
from .maybe_uninit import MaybeUninit
from .owned_pointer import OwnedPointer
