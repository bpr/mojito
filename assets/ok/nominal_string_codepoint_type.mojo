def main():
    var s = String("gé🙂")
    try:
        var g: Codepoint = s[codepoint=0]
        var e = s[codepoint=1]
        var face = s[codepoint=2]
        print(Int(g), Int(e), Int(face))
        print(g.is_ascii(), e.is_ascii())
        print(g.utf8_byte_length(), e.utf8_byte_length(), face.utf8_byte_length())
        print(g == g, g == e, g != e)
        print(g < e, face > e, g <= g, e >= face)
        print(g, e, face)
    except:
        print("unexpected")
    print("done")
