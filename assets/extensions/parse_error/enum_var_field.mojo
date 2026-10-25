# expect: enums do not support 'var' fields; store state in a case payload
# An enum's only state is its active case's payload.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case a
    var x: Int


def main():
    pass
