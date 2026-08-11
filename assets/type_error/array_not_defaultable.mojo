# Array is not Defaultable: it has no zero-argument constructor, so every
# instance is fully initialized at construction.
# expect: no constructor overload
def main():
    var a = Array[Int, 2]()
    print(len(a))
