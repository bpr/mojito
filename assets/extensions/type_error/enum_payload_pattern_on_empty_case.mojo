# expect: enum case 'off' has no associated value
# An enum case has no payload for a payload pattern to match.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Light:
    case on, off


def code(light: Light) -> Int:
    match light:
        case .off(_):
            return 3
        case _:
            return 4


def main():
    pass
