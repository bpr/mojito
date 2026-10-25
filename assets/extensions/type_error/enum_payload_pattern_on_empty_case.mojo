# expect: enum case 'off' has no associated value
# A case without a payload has nothing for a payload pattern to match.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Light:
    case on
    case off


def code(light: Light) -> Int:
    match light:
        case .off(_):
            return 3
        case _:
            return 4


def main():
    pass
