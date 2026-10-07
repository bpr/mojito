# A static method reached through an instance reads the instance's pack.
struct P[*Ts: AnyType]:
    var x: Int

    def __init__(out self):
        self.x = 1

    @staticmethod
    def has[T: AnyType]() -> Bool:
        return Self.Ts.contains[T]()

def main():
    var p = P[Int, String]()
    print(p.x, p.has[Int](), p.has[Bool]())
