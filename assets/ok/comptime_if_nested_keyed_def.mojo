# A nested generic `def` whose own `comptime if` keys on its type parameter
# specializes per call, from an inferred application as well as an explicit
# one, and its selected arm may call another compile-time-keyed `def`.

def show[T: Copyable](x: T):
    comptime if T == Int:
        print("int")
    else:
        print("other")

def main():
    def keyed[U: Copyable](y: U):
        comptime if U == Int:
            print("keyed int")
        else:
            print("keyed other")
        show(y)
    keyed(2)
    keyed(True)
    keyed[Int](7)
