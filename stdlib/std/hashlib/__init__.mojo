# Hashing with customizable algorithms: the `Hasher` protocol hash
# algorithms implement, the `Hashable` protocol hashable types conform to,
# `hash()`, and the default hasher aliases (`default_hasher` for runtime
# hashing, `default_comp_time_hasher` for compile-time hashing).

from .hash import Hashable, hash
from .hasher import Hasher, default_comp_time_hasher, default_hasher
