# expect: operator '+' is not defined for __module$std$string$Codepoint and IntLiteral[1]
def main():
    var s = String("go")
    try:
        print(s[codepoint=0] + 1)
    except:
        print("unexpected")
