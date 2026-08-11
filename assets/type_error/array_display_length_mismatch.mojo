# A fixed-size array display must supply exactly `length` elements.
# expect: fixed-size array display
def main():
    var a: Array[Int, 2] = [1, 2, 3]
    print(a[0])
