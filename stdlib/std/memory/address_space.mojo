# std.memory.address_space — upstream's `AddressSpace`, ported with these
# departures. Its `EnumLike` conformance is spelled through the bundled
# `SumType` members (`_sum_case_names`, `_get_sum_discriminant`), as
# `FastMathFlag`'s is; `_enum_case_types` and `_unsafe_get_enum_payload` are
# omitted. `GENERIC` and `GLOBAL` are spelled `AddressSpace(0)` and
# `AddressSpace(1)`, where upstream writes typed `#lit.struct` attributes
# (R2407). `__getattr_param__` and its `std._plugin` import are omitted
# (R2408), so an unknown `AddressSpace.NAME` reports a missing attribute.
"""Implements `AddressSpace`, the memory address space of a pointer.

You can import this API from the `memory` package. For example:

```mojo
from std.memory import AddressSpace
```
"""

from std.builtin.enum_like import EnumLike
from std.builtin.variadics import ParameterList


struct AddressSpace(
    EnumLike,
    Equatable,
    ImplicitlyCopyable,
    Intable,
    TrivialRegisterPassable,
    Writable,
):
    """Address space of the pointer.

    This type represents memory address spaces for both CPU and GPU targets.
    On CPUs, typically only GENERIC is used. On GPUs (NVIDIA/AMD), various
    address spaces provide access to different memory regions with different
    performance characteristics.
    """

    var _value: SIMDLength

    # CPU address space
    comptime GENERIC = AddressSpace(0)
    """Generic address space. Used for CPU memory and default GPU memory."""

    # GPU address spaces
    # See https://docs.nvidia.com/cuda/nvvm-ir-spec/#address-space
    # And https://llvm.org/docs/AMDGPUUsage.html#address-spaces
    comptime GLOBAL = AddressSpace(1)
    # Address space numbers are required to align with LLVM IR.
    """Global GPU memory address space."""
    comptime SHARED = AddressSpace(3)
    """Shared GPU memory address space (per thread block/workgroup)."""
    comptime CONSTANT = AddressSpace(4)
    """Constant GPU memory address space (read-only)."""
    comptime LOCAL = AddressSpace(5)
    """Local GPU memory address space (per thread, private)."""
    comptime SHARED_CLUSTER = AddressSpace(7)
    """Shared cluster GPU memory address space (NVIDIA-specific)."""
    comptime BUFFER_RESOURCE = AddressSpace(8)
    """Buffer resource GPU memory address space (AMD-specific)."""

    comptime _sum_case_names = ParameterList.of[
        "GENERIC".value,
        "GLOBAL".value,
        "SHARED".value,
        "CONSTANT".value,
        "LOCAL".value,
        "SHARED_CLUSTER".value,
        "BUFFER_RESOURCE".value,
    ].values
    comptime _enum_is_exhaustive = False

    @inline(.always)
    def _get_sum_discriminant(self) -> Int:
        match Int(self._value):
            case 0:
                return 0
            case 1:
                return 1
            case 3:
                return 2
            case 4:
                return 3
            case 5:
                return 4
            case 7:
                return 5
            case 8:
                return 6
            case _:
                return -1

    @always_inline("builtin")
    def __init__(out self, value: SIMDLength):
        """Initializes the address space from the underlying integral value.

        Args:
          value: The address space value.
        """
        self._value = value

    @always_inline("builtin")
    def value(self) -> SIMDLength:
        """The integral value of the address space.

        Returns:
          The integral value of the address space.
        """
        return self._value

    @always_inline("builtin")
    def __int__(self) -> Int:
        """The integral value of the address space.

        Returns:
          The integral value of the address space.
        """
        return Int(self._value)

    @always_inline("builtin")
    def __eq__(self, other: Self) -> Bool:
        """Checks if the two address spaces are equal.

        Args:
          other: The other address space value.

        Returns:
          True if the two address spaces are equal and False otherwise.
        """
        return self._value == other._value

    @inline(.nodebug)
    def write_to(self, mut writer: Some[Writer]):
        """Formats the address space to the provided Writer.

        Args:
            writer: The object to write to.
        """
        match self:
            case .GENERIC:
                writer.write("AddressSpace.GENERIC")
            case .GLOBAL:
                writer.write("AddressSpace.GLOBAL")
            case .SHARED:
                writer.write("AddressSpace.SHARED")
            case .CONSTANT:
                writer.write("AddressSpace.CONSTANT")
            case .LOCAL:
                writer.write("AddressSpace.LOCAL")
            case .SHARED_CLUSTER:
                writer.write("AddressSpace.SHARED_CLUSTER")
            case _:
                writer.write("AddressSpace(", Int(self.value()), ")")

    def write_repr_to(self, mut writer: Some[Writer]):
        """Write the string representation of the AddressSpace.

        Args:
            writer: The object to write to.
        """
        self.write_to(writer)
