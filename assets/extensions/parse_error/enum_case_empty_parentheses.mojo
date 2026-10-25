# expect: enum case must have the form 'case name' or 'case name(T, ...)'
# A case without a payload is written without parentheses: `case x()`
# matches neither form upstream accepts.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case empty()


def main():
    pass
