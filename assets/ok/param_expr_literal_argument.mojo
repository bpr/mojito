# `Array[Int, 3]` and `Array[Int, Int(3)]` are one type: a value argument takes
# the declared parameter type's ordinary literal conversion.
def main():
    var source: Array[Int, 3] = [1, 2, 3]
    var a: Array[Int, Int(3)] = source^
    print(a[2])
