# expect: '_get_enum_discriminant' is synthesized for enums and cannot be declared
# The `EnumLike` members an enum gets are generated; writing one is an error.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum E:
    case a

    def _get_enum_discriminant(self) -> Int:
        return 0


def main():
    pass
