# expect: a walrus yields its destination borrowed
# A walrus yields its destination borrowed, immutably, so it binds no `mut`
# parameter.
def bump(mut s: String):
    s += "!"


def main():
    var s = String("a")
    bump(s := String("b"))
    print(s)
