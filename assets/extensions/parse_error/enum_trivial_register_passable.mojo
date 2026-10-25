# expect: enums cannot conform to 'TrivialRegisterPassable'
# An enum's storage is never trivially register-passable.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E(TrivialRegisterPassable):
    case a


def main():
    pass
