# expect: 'blinking' is not a case of 'Light'
# A case pattern names one of the enum's cases.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Light:
    case on
    case off


def code(light: Light) -> Int:
    match light:
        case .blinking:
            return 2
        case _:
            return 4


def main():
    pass
