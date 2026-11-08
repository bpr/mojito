# expect: cannot use a dynamic value in operator argument
def main():
    var x = 3

    def mk() {imm x} -> Int:
        return x

    comptime if mk() > 2:
        print("big")
