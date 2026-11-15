# An `enum` case is a `comptime` value of the enum, read by name and by
# leading dot, never called. The enum is `ImplicitlyCopyable` without
# declaring it, so a case materializes at run time and a value copies
# implicitly. Cases listed on one line keep their order, two cases stay
# distinct, and an open enum keeps a fallback.
# `enum` is the declaration upstream's enum proposal gives cases without
# payloads, which the pin declares with `__enum` (`docs/non-goals.md`).


enum Light:
    case off
    case on, dimmed


enum Level:
    case low, high

    comptime _enum_is_exhaustive = False


def describe(light: Light) -> String:
    match light:
        case .off:
            return "off"
        case .on:
            return "on"
        case .dimmed:
            return "dimmed"


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
    print(describe(Light.off), Light.off._get_sum_discriminant())
    print(describe(pick(True)), pick(True)._get_sum_discriminant())
    var dimmed: Light = .dimmed
    var copy = dimmed
    print(describe(copy), copy._get_sum_discriminant())
    print(describe_level(Level.low), describe_level(.high))
