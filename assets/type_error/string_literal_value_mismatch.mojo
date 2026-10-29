# A literal of one text is no `StringLiteral` of another: its type is the
# application its text keys.
# expect: cannot implicitly convert 'StringLiteral["b"]' value to 'StringLiteral["a"]'
def main():
    var x: StringLiteral["a".value] = "b"
    print(x)
