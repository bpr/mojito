# Canonical implicit CPU collection prelude.
#
# The source linker loads this module for every linked program and binds these
# declarations under stable unqualified public identities. Implementations stay
# in their ordinary stdlib modules so explicit imports and aliases select the
# same declarations rather than creating a second collection universe.

from std.collections.array import Array
from std.collections.list import List
from std.format import Writable, Writer, repr
from std.format.tstring import TString
from std.string import (
    Codepoint,
    StaticString,
    String,
    StringSpan,
    StringSlice,
    atof,
    atol,
)
from std.collections.set import Set
from std.collections.dict import Dict
from std.collections.optional import Optional, OptionalReg
from std.builtin.len import Sized, SizedRaising, len
from std.builtin.comparable import Comparable, Equatable
from std.builtin.debug_assert import debug_assert
from std.builtin.bool import Bool, Boolable, all, any
from std.builtin.floatable import Floatable, FloatableRaising
from std.builtin.identifiable import Identifiable
from std.builtin.value import (
    Defaultable,
    RegisterPassable,
    TrivialRegisterPassable,
)
from std.builtin.int import Indexer, Intable, IntableRaising, index
from std.builtin.simd_length import SIMDLength
from std.builtin.dtype import DType
from std.simd import (
    SIMD,
    Byte,
    Float16,
    Float32,
    Float64,
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    Scalar,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    UInt,
)
from std.builtin.string_literal import StringLiteral
from std.builtin.int_literal import IntLiteral
from std.builtin.float_literal import FloatLiteral
from std.math import (
    Absable,
    Roundable,
    abs,
    divmod,
    max,
    min,
    pow,
    round,
)
from std.builtin.enum_like import EnumLike
from std.builtin.tuple import Tuple
from std.builtin.variadics import (
    ParameterList,
    TypeList,
    VariadicList,
    VariadicPack,
)
from std.range import range
from std.builtin.builtin_slice import Slice, slice
from std.iter import Iterable, IterableOwned, Iterator, StopIteration, next
from std.builtin.reversed import reversed
from std.memory.alloc import alloc
from std.span import Span
from std.hash import Hashable, hash
from std.io import RawFileDescriptor, FileHandle, open
from std.io.io import print

# Linked for its bodies, not for a name: the compiler calls these by symbol.
from std.builtin._stubs import (
    paramfor_has_next,
    paramfor_next_iter,
    paramfor_next_value,
)
# Every `enum` conforms to `__Enum`, by its linked name, as upstream's
# compiler finds the module implementing enums by path.
from std.builtin._enum import __Enum
