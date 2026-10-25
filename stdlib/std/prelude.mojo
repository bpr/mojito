# Canonical implicit CPU collection prelude.
#
# The source linker loads this module for every linked program and binds these
# declarations under stable unqualified public identities. Implementations stay
# in their ordinary stdlib modules so explicit imports and aliases select the
# same declarations rather than creating a second collection universe.

from std.collections.array import Array
from std.collections.list import List
from std.format import Writer
from std.format.tstring import TString
from std.string import Codepoint, String, StringSpan, atof, atol
from std.collections.set import Set
from std.collections.dict import Dict
from std.collections.optional import Optional, OptionalReg
from std.builtin.len import Sized, SizedRaising, len
from std.builtin.enum_like import EnumLike
from std.builtin.tuple import Tuple
from std.builtin.variadics import ParameterList, TypeList, VariadicList
from std.range import range
from std.builtin.builtin_slice import Slice, slice
from std.iter import Iterable, IterableOwned, Iterator, StopIteration, next
from std.builtin.reversed import reversed
from std.memory.alloc import alloc
from std.span import Span
from std.hash import hash
from std.io import RawFileDescriptor, FileHandle, open

# Linked for its bodies, not for a name: the compiler calls these by symbol.
from std._intrinsics import _pow_int
from std.builtin._stubs import (
    paramfor_has_next,
    paramfor_next_iter,
    paramfor_next_value,
)
# Every `enum` conforms to `__Enum`, by its linked name, as upstream's
# compiler finds the module implementing enums by path.
from std.builtin._enum import __Enum
