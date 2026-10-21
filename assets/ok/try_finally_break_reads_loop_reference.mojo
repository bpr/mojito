# A `break` out of a `try` whose `finally` reads the loop's reference into a
# named `List`: the list and its iterator stay alive through the `finally`,
# which runs before the loop's exit, as upstream's `lowerTryFinally` copies the
# `finally` before the `break`.
#
# Expected output:
#   body a
#   finally a
#   breaking
#   finally b
#   after


def main():
    var xs = List[String]()
    xs.append("a")
    xs.append("b")
    xs.append("c")
    for x in xs:
        try:
            if x == "b":
                print("breaking")
                break
            print("body", x)
        finally:
            print("finally", x)
    print("after")
