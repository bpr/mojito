# An `enum` that opts out of `ImplicitlyCopyable` cannot be copied
# implicitly: reading a case into a variable needs `materialize`. The pin
# reports "cannot materialize comptime value of type 'Explicit' to runtime
# because it is not 'ImplicitlyCopyable'".
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).
# expect: value of type 'Explicit' cannot be implicitly copied, it does not conform to 'ImplicitlyCopyable'


enum Explicit(not ImplicitlyCopyable):
    case one, two


def main():
    var explicit = Explicit.one
    _ = explicit^
