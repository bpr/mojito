# expect: expected StringLiteral, found String
# An un-annotated string binding materializes the nominal String, which does
# not narrow back to the compile-time StringLiteral.
def main():
    var s = "hi"
    var t: StringLiteral = s
    print(t)
