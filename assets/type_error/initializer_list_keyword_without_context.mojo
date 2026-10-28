# An initializer list names no type of its own: with a keyword entry and no
# contextual type, current Mojo has nothing to construct.
# expect: cannot emit initializer list without a contextual type
def main():
    var x = {a=1}
    print(1)
