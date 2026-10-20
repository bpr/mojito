# expect: operator '==' is not defined
# A literal pattern compares with `__eq__`, so a `String` literal against an
# `Int` subject is the ordinary missing-operator rejection.
def main():
    match 1:
        case "s":
            pass
