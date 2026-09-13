# Only a read versus an owned parameter distinguishes free-function overloads;
# a `mut` versus a read parameter of the same type is a redeclaration.
# expect: already declared
def k(t: Int):
    print("read")

def k(mut t: Int):
    print("mut")

def main():
    var x = 1
    k(x)
