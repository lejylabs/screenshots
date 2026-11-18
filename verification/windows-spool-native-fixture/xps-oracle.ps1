# Independent bounded XPS/OpenXPS image-page renderer. No XamlReader, URI fetch,
# package extraction, printer APIs or application print-layout helper is used.
function New-OwnedPrintImage([string]$Path) {
    $Bytes = [byte[]]::new(64*32*4)
    for ($Y=0;$Y -lt 32;$Y++) {for ($X=0;$X -lt 64;$X++) {
        $Color = if ($Y -lt 16) {if ($X -lt 32) {@(0,0,255,255)} else {@(0,255,0,128)}} else {if ($X -lt 32) {@(255,0,0,255)} else {@(6,5,4,0)}}
        [Array]::Copy([byte[]]$Color,0,$Bytes,($Y*64+$X)*4,4)
    }}
    $Bitmap = [Windows.Media.Imaging.BitmapSource]::Create(64,32,96,96,[Windows.Media.PixelFormats]::Bgra32,$null,$Bytes,256)
    $Encoder = [Windows.Media.Imaging.PngBitmapEncoder]::new()
    $Encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($Bitmap))
    $File = [IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try {$Encoder.Save($File)} finally {$File.Dispose()}
}
function Read-OwnedXml($Entry) {
    if ($Entry.Length -gt 1048576) {throw 'XPS XML part exceeds 1 MiB'}
    $Stream=$Entry.Open()
    $Settings=[Xml.XmlReaderSettings]::new();$Settings.DtdProcessing='Prohibit';$Settings.XmlResolver=$null;$Settings.MaxCharactersInDocument=1048576
    $Reader=[Xml.XmlReader]::Create($Stream,$Settings)
    try {$Xml=[Xml.XmlDocument]::new();$Xml.XmlResolver=$null;$Xml.Load($Reader);return ,$Xml} finally {$Reader.Dispose();$Stream.Dispose()}
}
function Convert-OwnedNumbers([string]$Text,[int]$Count) {
    $Parts=@($Text -split '[, ]+' | Where-Object {$_})
    if ($Parts.Count -ne $Count) {throw 'Unexpected numeric XPS attribute'}
    $Numbers=@($Parts | ForEach-Object {[double]::Parse($_,[Globalization.CultureInfo]::InvariantCulture)})
    foreach ($Number in $Numbers) {if ([double]::IsNaN($Number) -or [double]::IsInfinity($Number) -or [math]::Abs($Number) -gt 1000000) {throw 'Invalid/unbounded XPS numeric attribute'}}
    return ,$Numbers
}
function Resolve-OwnedPart([string]$Base,[string]$Reference,$Entries) {
    if (-not $Reference -or $Reference -match '[:\\?#{}]' -or $Reference.StartsWith('//')) {throw 'Only internal fixed XPS image references are allowed'}
    $BaseUri=[uri]::new('https://owned.invalid/'+$Base)
    $Uri=[uri]::new($BaseUri,$Reference)
    if ($Uri.Host -ne 'owned.invalid') {throw 'External XPS resource forbidden'}
    $Name=[uri]::UnescapeDataString($Uri.AbsolutePath.TrimStart('/'))
    if (-not $Entries.ContainsKey($Name)) {throw 'Missing package-internal XPS part'}
    return $Name
}
function New-OwnedBrush($Node,[string]$PageName,$Entries,$Budget) {
    if ($Node.LocalName -ne 'ImageBrush') {throw 'Unexpected XPS path fill; image brush required'}
    if ($Node.NamespaceURI -notin @('http://schemas.microsoft.com/xps/2005/06','http://schemas.openxps.org/oxps/v1.0')) {throw 'Unsupported XPS ImageBrush namespace'}
    foreach ($Child in $Node.ChildNodes) {
        if ($Child.NodeType -eq [Xml.XmlNodeType]::Element -or ($Child.NodeType -in @([Xml.XmlNodeType]::Text,[Xml.XmlNodeType]::CDATA) -and -not [string]::IsNullOrWhiteSpace($Child.Value))) {throw 'Unsupported XPS ImageBrush child/property markup'}
    }
    foreach ($Attribute in $Node.Attributes) {
        if ($Attribute.Prefix -eq 'xmlns' -or $Attribute.Name -eq 'xmlns') {continue}
        if ($Attribute.Name -notin @('ImageSource','Viewbox','Viewport','ViewboxUnits','ViewportUnits','TileMode','Transform','Opacity')) {throw ('Unsupported XPS ImageBrush attribute: '+$Attribute.Name)}
    }
    $Name=Resolve-OwnedPart $PageName ($Node.GetAttribute('ImageSource')) $Entries
    $Entry=$Entries[$Name]
    if ($Entry.Length -gt 16777216) {throw 'Embedded image exceeds encoded bound'}
    $Input=$Entry.Open();$Memory=[IO.MemoryStream]::new()
    try {
        $Input.CopyTo($Memory);$Memory.Position=0
        $Decoder=[Windows.Media.Imaging.BitmapDecoder]::Create($Memory,[Windows.Media.Imaging.BitmapCreateOptions]::PreservePixelFormat,[Windows.Media.Imaging.BitmapCacheOption]::OnLoad)
        if ($Decoder.Frames.Count -ne 1) {throw 'Embedded image is not a single raster'}
        $Frame=$Decoder.Frames[0]
        $Bytes=[long]$Frame.PixelWidth*$Frame.PixelHeight*4
        $Budget.decoded += $Bytes
        if ($Frame.PixelWidth -le 0 -or $Frame.PixelHeight -le 0 -or $Budget.decoded -gt 33554432) {throw 'Embedded decoded image budget exceeded'}
        $Frame.Freeze()
    } finally {$Input.Dispose();$Memory.Dispose()}
    $Brush=[Windows.Media.ImageBrush]::new($Frame)
    $Brush.Stretch=[Windows.Media.Stretch]::Fill
    foreach ($Field in @('Viewbox','Viewport')) {
        if (-not $Node.HasAttribute($Field)) {throw 'Explicit XPS image rectangles required'}
        $N=Convert-OwnedNumbers ($Node.GetAttribute($Field)) 4
        $Brush.$Field=[Windows.Rect]::new($N[0],$N[1],$N[2],$N[3])
    }
    foreach ($Field in @('ViewboxUnits','ViewportUnits')) {
        $Value=$Node.GetAttribute($Field)
        if ($Value -notin @('Absolute','RelativeToBoundingBox')) {throw 'Explicit XPS image units required'}
        $Brush.$Field=[Windows.Media.BrushMappingMode]::$Value
    }
    if ($Node.HasAttribute('TileMode') -and $Node.GetAttribute('TileMode') -ne 'None') {throw 'Tiled image brush is outside the fixture contract'}
    if ($Node.HasAttribute('Opacity')) {$Brush.Opacity=(Convert-OwnedNumbers ($Node.GetAttribute('Opacity')) 1)[0]}
    if ($Node.HasAttribute('Transform')) {$Brush.Transform=[Windows.Media.MatrixTransform]::new([Windows.Media.Matrix]::Parse($Node.GetAttribute('Transform')))}
    $Budget.images++
    return $Brush
}
function New-OwnedVisual($Node,[string]$PageName,$Entries,$Budget,[int]$Depth=0) {
    $Budget.nodes++
    if ($Depth -gt 16 -or $Budget.nodes -gt 128) {throw 'XPS visual tree exceeds fixture budget'}
    if ($Node.NamespaceURI -notin @('http://schemas.microsoft.com/xps/2005/06','http://schemas.openxps.org/oxps/v1.0')) {throw 'Unexpected XPS namespace'}
    if ($Node.LocalName -eq 'FixedPage') {
        if (-not $Node.HasAttribute('Width') -or -not $Node.HasAttribute('Height')) {throw 'FixedPage requires explicit dimensions'}
        $DeclaredWidth=(Convert-OwnedNumbers $Node.GetAttribute('Width') 1)[0]
        $DeclaredHeight=(Convert-OwnedNumbers $Node.GetAttribute('Height') 1)[0]
        if ($DeclaredWidth -le 0 -or $DeclaredHeight -le 0) {throw 'FixedPage requires finite positive dimensions'}
    }
    if ($Node.LocalName -in @('FixedPage','Canvas')) {$Visual=[Windows.Controls.Canvas]::new()} elseif ($Node.LocalName -eq 'Path') {$Visual=[Windows.Shapes.Path]::new()} else {throw ('Unsupported XPS visual: '+$Node.LocalName)}
    foreach ($Attribute in $Node.Attributes) {
        if ($Attribute.Prefix -eq 'xmlns' -or $Attribute.Name -eq 'xmlns' -or $Attribute.Name -eq 'xml:lang') {continue}
        switch ($Attribute.Name) {
            'Width' {$Visual.Width=(Convert-OwnedNumbers $Attribute.Value 1)[0]}
            'Height' {$Visual.Height=(Convert-OwnedNumbers $Attribute.Value 1)[0]}
            'RenderTransform' {$Visual.RenderTransform=[Windows.Media.MatrixTransform]::new([Windows.Media.Matrix]::Parse($Attribute.Value))}
            'Clip' {$Visual.Clip=[Windows.Media.Geometry]::Parse($Attribute.Value)}
            'Data' {if ($Node.LocalName -ne 'Path') {throw 'Geometry on nonpath'};$Visual.Data=[Windows.Media.Geometry]::Parse($Attribute.Value)}
            'Fill' {if ($Node.LocalName -ne 'Path' -or $Attribute.Value -cnotmatch '^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$') {throw 'Only literal XPS path colors supported'};$Visual.Fill=[Windows.Media.BrushConverter]::new().ConvertFromInvariantString($Attribute.Value)}
            'Opacity' {$Visual.Opacity=(Convert-OwnedNumbers $Attribute.Value 1)[0]}
            default {throw ('Unsupported XPS visual attribute: '+$Attribute.Name)}
        }
    }
    foreach ($Child in $Node.ChildNodes) {
        if ($Child.NodeType -ne [Xml.XmlNodeType]::Element) {continue}
        if ($Child.NamespaceURI -cne $Node.NamespaceURI) {throw 'Unsupported XPS child/property namespace'}
        if ($Child.LocalName -eq 'Path.Fill' -and $Node.LocalName -eq 'Path') {
            $Fills=@($Child.ChildNodes | Where-Object {$_.NodeType -eq [Xml.XmlNodeType]::Element})
            if ($Fills.Count -ne 1) {throw 'One literal image fill required'}
            if ($Fills[0].NamespaceURI -cne $Node.NamespaceURI) {throw 'Unsupported XPS ImageBrush namespace'}
            $Visual.Fill=New-OwnedBrush $Fills[0] $PageName $Entries $Budget
        } elseif ($Node.LocalName -in @('FixedPage','Canvas')) {
            $Visual.Children.Add((New-OwnedVisual $Child $PageName $Entries $Budget ($Depth+1))) | Out-Null
        } else {throw ('Unsupported XPS property/child: '+$Child.LocalName)}
    }
    return $Visual
}
function Test-OwnedXpsOutput([string]$Path,[ValidateSet('letter','a4')][string]$Paper='letter') {
    $File=[IO.File]::Open($Path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None)
    $Archive=[IO.Compression.ZipArchive]::new($File,[IO.Compression.ZipArchiveMode]::Read,$true)
    try {
        if ($File.Length -gt 16777216 -or $Archive.Entries.Count -gt 128) {throw 'Output package exceeds bound'}
        $Entries=@{};$Total=0L;$Pages=@();$Sequences=@();$Documents=@()
        foreach ($Entry in $Archive.Entries) {
            $Total += $Entry.Length
            if ($Total -gt 67108864 -or $Entry.FullName -match '(^/|\\|(^|/)\.\.(/|$))' -or $Entries.ContainsKey($Entry.FullName)) {throw 'Invalid, duplicate or oversized output package part'}
            $Entries[$Entry.FullName]=$Entry
            if ($Entry.FullName -match '\.(xml|rels|fpage|fdoc|fdseq)$' -or $Entry.FullName -eq '[Content_Types].xml') {
                $Xml=Read-OwnedXml $Entry
                if ($Xml.SelectNodes('//*[@TargetMode="External"]').Count -or $Xml.SelectNodes('//*[local-name()="Glyphs"]').Count) {throw 'External relationship or unexpected text in owned raster output'}
                switch ($Xml.DocumentElement.LocalName) {
                    'FixedPage' {$Pages+=@([pscustomobject]@{name=$Entry.FullName;xml=$Xml})}
                    'FixedDocumentSequence' {$Sequences+=@([pscustomobject]@{name=$Entry.FullName;xml=$Xml})}
                    'FixedDocument' {$Documents+=@([pscustomobject]@{name=$Entry.FullName;xml=$Xml})}
                }
            }
        }
        if ($Pages.Count -ne 1 -or $Sequences.Count -ne 1 -or $Documents.Count -ne 1 -or -not $Entries.ContainsKey('_rels/.rels') -or -not $Entries.ContainsKey('[Content_Types].xml')) {throw 'Exactly one XPS sequence/document/page and OPC roots required'}
        $RootRelations=Read-OwnedXml $Entries['_rels/.rels']
        $RootFixed=@($RootRelations.SelectNodes('//*[local-name()="Relationship"]') | Where-Object {$_.GetAttribute('Type') -match '/fixedrepresentation$'})
        if ($RootFixed.Count -ne 1 -or (Resolve-OwnedPart '' ($RootFixed[0].GetAttribute('Target')) $Entries) -cne $Sequences[0].name) {throw 'Wrong OPC fixed representation'}
        $DocRefs=@($Sequences[0].xml.SelectNodes('//*[local-name()="DocumentReference"]'))
        $PageRefs=@($Documents[0].xml.SelectNodes('//*[local-name()="PageContent"]'))
        if ($DocRefs.Count -ne 1 -or $PageRefs.Count -ne 1 -or (Resolve-OwnedPart $Sequences[0].name ($DocRefs[0].GetAttribute('Source')) $Entries) -cne $Documents[0].name -or (Resolve-OwnedPart $Documents[0].name ($PageRefs[0].GetAttribute('Source')) $Entries) -cne $Pages[0].name) {throw 'XPS page is not the sole referenced document page'}
        $Budget=@{nodes=0;images=0;decoded=0L}
        $Visual=New-OwnedVisual ($Pages[0].xml.DocumentElement) $Pages[0].name $Entries $Budget
        $PageWidth=if ($Paper -eq 'letter') {816.0} else {210.0*96.0/25.4}
        $PageHeight=if ($Paper -eq 'letter') {1056.0} else {297.0*96.0/25.4}
        if ([math]::Abs($Visual.Width-$PageWidth) -gt 0.5 -or [math]::Abs($Visual.Height-$PageHeight) -gt 0.5 -or $Budget.images -lt 1) {throw 'Expected one requested portrait page and embedded image'}
        $Width=[int][math]::Ceiling($PageWidth);$Height=[int][math]::Ceiling($PageHeight)
        $Size=[Windows.Size]::new($Width,$Height);$Visual.Measure($Size);$Visual.Arrange([Windows.Rect]::new($Size));$Visual.UpdateLayout()
        $Rendered=[Windows.Media.Imaging.RenderTargetBitmap]::new($Width,$Height,96,96,[Windows.Media.PixelFormats]::Pbgra32)
        $Rendered.Render($Visual)
        $Pixels=[byte[]]::new($Width*$Height*4);$Rendered.CopyPixels($Pixels,$Width*4,0)
        $Left=$Width;$Top=$Height;$Right=-1;$Bottom=-1
        for ($Y=0;$Y -lt $Height;$Y++) {for ($X=0;$X -lt $Width;$X++) {
            $At=($Y*$Width+$X)*4;$White=255-$Pixels[$At+3]
            if ([math]::Min([math]::Min($Pixels[$At]+$White,$Pixels[$At+1]+$White),$Pixels[$At+2]+$White) -lt 220) {
                $Left=[math]::Min($Left,$X);$Top=[math]::Min($Top,$Y);$Right=[math]::Max($Right,$X);$Bottom=[math]::Max($Bottom,$Y)
            }
        }}
        # Independent 2:1 fit with 24/96-inch border; no app-core layout call.
        $FitWidth=$PageWidth-48;$FitHeight=$FitWidth/2;$FitTop=($PageHeight-$FitHeight)/2
        foreach ($Pair in @(@($Left,24),@($Top,$FitTop),@($Right,24+$FitWidth-1),@($Bottom,$FitTop+$FitHeight-1))) {if ([math]::Abs($Pair[0]-$Pair[1]) -gt 2) {throw 'Native driver output has wrong physical fit/bounds'}}
        foreach ($Sample in @(@(0.25,0.25,0,0,255),@(0.75,0.25,127,255,127),@(0.25,0.75,255,0,0),@(0.75,0.75,255,255,255))) {
            $X=[int](24+$Sample[0]*$FitWidth);$Y=[int]($FitTop+$Sample[1]*$FitHeight)
            $At=($Y*$Width+$X)*4;$White=255-$Pixels[$At+3]
            for ($C=0;$C -lt 3;$C++) {if ([math]::Abs($Pixels[$At+$C]+$White-$Sample[$C+2]) -gt 12) {throw 'Native driver output quadrant/color/alpha/orientation mismatch'}}
        }
        $File.Position=0;$Hasher=[Security.Cryptography.SHA256]::Create()
        try {$Hash=[BitConverter]::ToString($Hasher.ComputeHash($File)).Replace('-','')} finally {$Hasher.Dispose()}
        return [ordered]@{page_count=1;paper=$Paper;page_96dpi=@($PageWidth,$PageHeight);observed_nonwhite_bounds=@($Left,$Top,$Right,$Bottom);color_tolerance=12;bounds_tolerance=2;render='strict bounded XPS/OpenXPS image subset through WPF';embedded_images=$Budget.images;decoded_image_bytes=$Budget.decoded;output_sha256=$Hash}
    } finally {$Archive.Dispose();$File.Dispose()}
}

function Test-OwnedOracleControls([string]$Directory,[string]$InputImage) {
    foreach ($Mode in @('golden','shifted-fit','wrong-color','extra-page','nested-brush-transform','foreign-brush-namespace','missing-width','missing-height')) {
        $Path=Join-Path $Directory ('oracle-control-'+$Mode+'.xps')
        $File=[IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
        $Zip=[IO.Compression.ZipArchive]::new($File,[IO.Compression.ZipArchiveMode]::Create,$true)
        try {
            $Shift=if ($Mode -eq 'shifted-fit') {32} else {0}
            $X=24+$Shift;$Right=792+$Shift
            $Parts=@{
                '[Content_Types].xml'='<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="png" ContentType="image/png"/></Types>'
                '_rels/.rels'='<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="r1" Type="http://schemas.microsoft.com/xps/2005/06/fixedrepresentation" Target="sequence.fdseq"/></Relationships>'
                'sequence.fdseq'='<FixedDocumentSequence xmlns="http://schemas.microsoft.com/xps/2005/06"><DocumentReference Source="document.fdoc"/></FixedDocumentSequence>'
                'document.fdoc'='<FixedDocument xmlns="http://schemas.microsoft.com/xps/2005/06"><PageContent Source="page.fpage"/></FixedDocument>'
                'page.fpage'="<FixedPage xmlns=`"http://schemas.microsoft.com/xps/2005/06`" Width=`"816`" Height=`"1056`"><Path Data=`"M$X,336 L$Right,336 L$Right,720 L$X,720 Z`"><Path.Fill><ImageBrush ImageSource=`"image.png`" Viewbox=`"0,0,64,32`" Viewport=`"$X,336,768,384`" ViewboxUnits=`"Absolute`" ViewportUnits=`"Absolute`"/></Path.Fill></Path></FixedPage>"
            }
            if ($Mode -eq 'missing-width') {$Parts['page.fpage']=$Parts['page.fpage'].Replace(' Width="816"','')}
            if ($Mode -eq 'missing-height') {$Parts['page.fpage']=$Parts['page.fpage'].Replace(' Height="1056"','')}
            if ($Mode -eq 'extra-page') {$Parts['extra.fpage']=$Parts['page.fpage']}
            if ($Mode -eq 'nested-brush-transform') {$Parts['page.fpage']=$Parts['page.fpage'].Replace('/></Path.Fill>','><ImageBrush.Transform><MatrixTransform Matrix="1,0,0,1,32,0"/></ImageBrush.Transform></ImageBrush></Path.Fill>')}
            if ($Mode -eq 'foreign-brush-namespace') {$Parts['page.fpage']=$Parts['page.fpage'].Replace('<ImageBrush ','<ImageBrush xmlns="urn:owned-invalid-brush" ')}
            foreach ($Name in $Parts.Keys) {
                $Stream=$Zip.CreateEntry($Name).Open();$Writer=[IO.StreamWriter]::new($Stream,[Text.UTF8Encoding]::new($false))
                try {$Writer.Write($Parts[$Name])} finally {$Writer.Dispose()}
            }
            $Image=$Zip.CreateEntry('image.png').Open()
            try {
                if ($Mode -eq 'wrong-color') {
                    $Wrong=[byte[]]::new(64*32*4)
                    for ($At=3;$At -lt $Wrong.Length;$At+=4) {$Wrong[$At]=255}
                    $Bitmap=[Windows.Media.Imaging.BitmapSource]::Create(64,32,96,96,[Windows.Media.PixelFormats]::Bgra32,$null,$Wrong,256)
                    $Encoder=[Windows.Media.Imaging.PngBitmapEncoder]::new();$Encoder.Frames.Add([Windows.Media.Imaging.BitmapFrame]::Create($Bitmap));$Encoder.Save($Image)
                } else {$Source=[IO.File]::OpenRead($InputImage);try {$Source.CopyTo($Image)} finally {$Source.Dispose()}}
            } finally {$Image.Dispose()}
        } finally {$Zip.Dispose();$File.Dispose()}
        $Rejected=$false;$Failure=''
        try {$null=Test-OwnedXpsOutput $Path} catch {$Rejected=$true;$Failure=$_.Exception.Message} finally {[IO.File]::Delete($Path)}
        if (($Mode -eq 'golden' -and $Rejected) -or ($Mode -ne 'golden' -and -not $Rejected)) {throw ('Independent output oracle self-control failed: '+$Mode)}
        if ($Mode -in @('missing-width','missing-height') -and $Failure -notmatch 'FixedPage requires explicit dimensions') {throw 'Missing page dimensions control missed its required rejection boundary'}
        if ($Mode -eq 'nested-brush-transform' -and $Failure -notmatch 'ImageBrush child/property markup') {throw 'Nested brush transform control missed its required rejection boundary'}
        if ($Mode -eq 'foreign-brush-namespace' -and $Failure -notmatch 'ImageBrush namespace') {throw 'Foreign brush namespace control missed its required rejection boundary'}
    }
    return [ordered]@{golden_accepted=$true;shifted_fit_rejected=$true;wrong_color_rejected=$true;extra_page_rejected=$true;nested_brush_transform_rejected=$true;foreign_brush_namespace_rejected=$true;missing_width_rejected=$true;missing_height_rejected=$true}
}
