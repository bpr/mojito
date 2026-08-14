# expect: '_subtree' is a terminal origin projection
# Nothing projects below the conservative subtree form — it already
# designates the base and every descendant.
@fieldwise_init
struct Watch[mut: Bool, //, origin: Origin[mut=mut]]:
    var data: Pointer[Int, Self.origin._subtree._get_owned_interior["x"]]

def main():
    print("no")
