# expect: enum case 'red' is a value, not a constructor; write 'Color.red' without parentheses
# An enum case is a value: parentheses would construct a payload, and an
# enum case has none.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Color:
    case red, green


def main():
    var color = Color.red()
