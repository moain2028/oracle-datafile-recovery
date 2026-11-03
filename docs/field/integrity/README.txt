ODR - integrity folder
======================

This folder proves that the files around it are exactly what the ODR build system produced.
It contains no secret: only public data (the private signing key never leaves the build system).

  SHA256SUMS            SHA-256 of every other file in this package (relative paths)
  SHA256SUMS.p7s        signature of SHA256SUMS by the ODR code-signing key
  ODR-code-signing.cer  the public ODR certificate (CN=ODR Oracle Datafile Recovery, O=moain2028)
  SIGNER.txt            its thumbprint - the one value every check compares against
  VERSION.txt           version, source commit and build time of this package
  verify.ps1            Windows check:  powershell -ExecutionPolicy Bypass -File integrity\verify.ps1
  verify.sh             Linux check:    bash integrity/verify.sh
  trust-certificate.ps1 optional: trust the certificate for your Windows account (undo: -Remove)

odr.exe and odr-corrupt.exe are Authenticode-signed (SHA-256, RFC 3161 timestamp) with this
certificate. It is self-issued (free, not bought from a certificate authority), so Windows says
"unknown publisher" / SmartScreen may warn until you run trust-certificate.ps1. The signature
still detects ANY change to the program: verify.ps1 checks it without installing anything.

UPDATING: unzip a newer package over this folder (choose "replace all"). integrity\ is replaced
with the new version's lists; files that the package does not ship (your outputs, logs) are not
touched and not checked. Run verify again after updating.

-----------------------------------------------------------------------------------------------
مجلد السلامة (integrity)

يثبت أن ملفات الحزمة هي نفسها التي أنتجها نظام البناء، بلا أي تعديل.
لا يحتوي أسراراً: كله عام. المفتاح الخاص للتوقيع لا يغادر نظام البناء أبداً.

  SHA256SUMS            بصمة SHA-256 لكل ملف في الحزمة
  SHA256SUMS.p7s        توقيع قائمة البصمات بمفتاح ODR
  ODR-code-signing.cer  الشهادة العامة لـ ODR
  SIGNER.txt            بصمة الشهادة: القيمة التي يُقارَن بها كل شيء
  verify.ps1 / .sh      أداة الفحص (ويندوز / لينكس)
  trust-certificate.ps1 اختياري: اجعل ويندوز يثق بالشهادة لحسابك (للتراجع: -Remove)

البرنامج موقَّع رقمياً بشهادة ذاتية مجانية، لذلك قد يقول ويندوز «ناشر غير معروف» حتى تشغّل
trust-certificate.ps1. التوقيع مع ذلك يكشف أي تغيير في البرنامج، وverify.ps1 يفحصه بلا تثبيت.

التحديث: فكّ ضغط النسخة الجديدة فوق نفس المجلد واختر «استبدال الكل». يُستبدل integrity بقوائم
النسخة الجديدة، ولا تُلمس ملفاتك (المخرجات والسجلات). شغّل الفحص مرة أخرى بعد التحديث.
