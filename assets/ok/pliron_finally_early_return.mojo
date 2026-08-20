# Early returns cross a `finally` on the way out — the pending value
# survives the finalbody, for scalar and String returns alike.
def pick(x: Int) -> Int:
    try:
        if x > 0:
            return 1
        return 2
    finally:
        print("fin", x)

def label(x: Int) -> String:
    try:
        if x > 0:
            return "pos"
        return "nonpos"
    finally:
        print("checked", x)

def main():
    print(pick(5))
    print(pick(-1))
    print(label(5))
    print(label(-2))
