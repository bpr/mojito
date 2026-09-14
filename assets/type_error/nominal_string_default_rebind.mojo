# expect: 'StringLiteral[_]' is not concrete
# An un-annotated string binding materializes the nominal String, and the bare
# `StringLiteral` spelling is not concrete outside a parameter annotation.
def main():
    var s = "hi"
    var t: StringLiteral = s
    print(t)
