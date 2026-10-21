# A fixed-length sequence destructuring raises `list pattern does not match
# value length` before any store, so a wrong length is caught by `try` and
# leaves an assigned target as it was.
def main():
    var values: List[Int] = [1, 2, 3]
    try:
        var [a, b] = values
        print(a, b)
    except e:
        print("error:", e)
    try:
        var [a, b, c, d] = values
        print(a, b, c, d)
    except e:
        print("error:", e)
    var x = 0
    var y = 0
    try:
        var z = 0
        [x, y, z] = values
        print(x, y, z)
        [x, y] = values
        print("unreachable")
    except e:
        print("error:", e, x, y)
