# `continue` and `break` escaping a `try` body inside a loop run the
# `finally` on the way out.
def main():
    var i = 0
    while i < 5:
        i = i + 1
        try:
            if i == 2:
                continue
            if i == 4:
                break
            print("body", i)
        finally:
            print("fin", i)
    print("after", i)
