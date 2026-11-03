# expect: has no method '__hash__'
# Upstream's `StringLiteral` is not `Hashable` and declares no `__hash__`:
# `hash("abc")` hashes the `String` the literal materializes to, but the
# literal itself has no member to call.
from std.hash.hasher import default_hasher


def main():
    var h = default_hasher()
    "abc".__hash__(h)
    print(h^.finish())
