# Third-party licenses

ZipShrink is MIT licensed and written in C with no C runtime library. The
only third-party code in the programs is miniz's deflate and inflate:

* **miniz 3.0.2** (`miniz_tdef.c`, `miniz_tinfl.c` and their headers, in
  `vendor/miniz/`), https://github.com/richgel999/miniz, MIT License:

```
Copyright 2013-2014 RAD Game Tools and Valve Software
Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC

All Rights Reserved.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

Everything else (CRC-32, SHA-256, UTF-8/UTF-16 conversion, the ZIP reader and
writer, the journal, the user interface) is ZipShrink's own code under its
MIT License. The installers are made with NSIS (zlib License) and msitools'
wixl (LGPL), which are build tools only and not part of the programs; the
NSIS installer contains NSIS's own small stub program under the zlib License.
