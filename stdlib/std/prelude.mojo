# Canonical implicit CPU collection prelude.
#
# The source linker loads this module for every linked program and binds these
# declarations under stable unqualified public identities. Implementations stay
# in their ordinary stdlib modules so explicit imports and aliases select the
# same declarations rather than creating a second collection universe.

from std.collections.list import List
from std.collections.set import Set
from std.collections.dict import Dict
from std.collections.tuple import Tuple
from std.range import Range, range
