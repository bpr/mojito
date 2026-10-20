# Same-indent and indented `case` blocks, same-line case bodies, and a
# nested `match`.
def main():
    var n = 2
    match n:
    case 1: print("one")
    case 2: print("two")
    case _: print("other")
    match n:
        case 1:
            print("one")
        case 2:
            match n + 1:
                case 3:
                    print("three")
                case _:
                    print("nope")
        case _:
            print("other")
