# expect: '_subtree' is a terminal origin projection
@fieldwise_init
struct Watch[mut: Bool, //, origin: Origin[mut=mut]]:
    var data: Pointer[Int, Self.origin._subtree._subtree]

def main():
    print("no")
