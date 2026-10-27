# A capitalized display or reflection binding read in a bracket operand.


@fieldwise_init
struct P:
    var x: Int
    var y: Int


def main():
    comptime L = [1, 2]
    print(materialize[L[1]]())
    comptime NAMES = reflect[P].field_names()
    print(materialize[NAMES[0]]())
