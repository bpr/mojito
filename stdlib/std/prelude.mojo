# Canonical implicit CPU collection prelude.
#
# The source linker loads this module for every linked program and binds these
# declarations under stable unqualified public identities. Implementations stay
# in their ordinary stdlib modules so explicit imports and aliases select the
# same declarations rather than creating a second collection universe.

from std.collections.array import Array
from std.collections.list import List
from std.format.tstring import TString
from std.string import Codepoint, String, StringSpan, atof, atol
from std.collections.set import Set
from std.collections.dict import Dict
from std.optional import Optional
from std.collections.tuple import Tuple
from std.range import range
from std.iterable import next
from std.builtin.reversed import reversed
from std.memory import alloc
from std.span import Span
from std.hashlib import hash
