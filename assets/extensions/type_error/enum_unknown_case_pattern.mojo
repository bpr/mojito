# expect: 'blinking' is not a case of 'Light'
# A case pattern names one of the enum's cases.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


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
