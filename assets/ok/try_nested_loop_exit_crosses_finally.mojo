# A `continue` and a `break` from a `try` nested in another `try`'s body, to
# the loop inside that body: each crosses the inner `finally` only, and the
# outer `finally` runs once, after the loop.
#
# Expected output:
#   body 0
#   inner finally 0
#   inner finally 1
#   body 2
#   inner finally 2
#   inner finally 3
#   outer finally


def main() raises:
    try:
        for i in range(4):
            try:
                if i == 1:
                    continue
                if i == 3:
                    break
                print("body", i)
            finally:
                print("inner finally", i)
    finally:
        print("outer finally")
