# std.memory.pointer — upstream's `Pointer` aliases, ported with these
# departures. The `Pointer` struct itself is not declared yet: the compiler
# answers `Pointer[...]` by rule (R2414 declares it here; the compiler
# already folds the struct's applications into its own pointer type). `@stable` and `@deprecated` on each alias are omitted,
# since a decorator may not precede a `comptime` declaration yet (R1925), so
# the three `Optional*Pointer` aliases do not warn as the pin does.
"""Implements `Pointer`, Mojo's primary pointer type for indirect memory
access.

`Pointer` is safe when constructed from and used to access an existing
value. It also provides `unsafe_`-prefixed methods for working with
dynamically-allocated or uninitialized memory; the caller is responsible
for tracking initialization state and memory ownership.

You can import these APIs from the `memory` package. For example:

```mojo
from std.memory import Pointer
```
"""

from std.collections.optional import Optional
from std.memory.address_space import AddressSpace


comptime MutPointer[
    T: AnyType,
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[T, origin, address_space=address_space]
"""A mutable pointer.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime ImmPointer[
    T: AnyType,
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[T, origin, address_space=address_space]
"""An immutable pointer.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalPointer[
    mut: Bool,
    //,
    T: AnyType,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[Pointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) `Pointer`.

Use `Optional[Pointer[T, origin]]` instead.

Parameters:
    mut: The mutability of the pointer.
    T: The type of the pointee.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalMutPointer[
    T: AnyType,
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[MutPointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) mutable `Pointer`.

Use `Optional[MutPointer[T, origin]]` instead.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""


comptime OptionalImmPointer[
    T: AnyType,
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = Optional[ImmPointer[T, origin, address_space=address_space]]
"""Deprecated: An optional (nullable) immutable `Pointer`.

Use `Optional[ImmPointer[T, origin]]` instead.

Parameters:
    T: The pointee type.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime OpaquePointer[
    mut: Bool,
    //,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
] = Pointer[NoneType, origin, address_space=address_space]
"""An opaque pointer, equivalent to the C `(const) void*` type.

Parameters:
    mut: Whether the pointer is mutable.
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime MutOpaquePointer[
    origin: MutOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = OpaquePointer[origin, address_space=address_space]
"""A mutable opaque pointer, equivalent to the C `void*` type.

Parameters:
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""

comptime ImmOpaquePointer[
    origin: ImmOrigin,
    *,
    address_space: AddressSpace = .GENERIC,
] = OpaquePointer[origin, address_space=address_space]
"""An immutable opaque pointer, equivalent to the C `const void*` type.

Parameters:
    origin: The origin of the pointer.
    address_space: The address space of the pointer.
"""
