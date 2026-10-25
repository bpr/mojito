# An `enum` case without a payload is a `comptime` member of the enum,
# read by name and by leading dot, as upstream generates it; the enum is
# `ImplicitlyCopyable` so the member materializes at run time, as the pin
# requires. Two such cases stay distinct, and an open enum keeps a fallback.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Light(ImplicitlyCopyable):
    case off
    case on
    case dimmed(Int)


enum Level(ImplicitlyCopyable):
    case low
    case high

    comptime _enum_is_exhaustive = False


def describe(light: Light) -> String:
    match light:
        case .off:
            return "off"
        case .on:
            return "on"
        case .dimmed(level):
            return "dimmed " + String(level)


def describe_level(level: Level) -> String:
    match level:
        case .low:
            return "low"
        case .high:
            return "high"
        case _:
            return "other"


def pick(bright: Bool) -> Light:
    if bright:
        return .on
    return Light.off


def main():
    print(describe(Light.off), Light.off._get_enum_discriminant())
    print(describe(pick(True)), pick(True)._get_enum_discriminant())
    print(describe(.dimmed(3)))
    print(describe_level(Level.low), describe_level(.high))
