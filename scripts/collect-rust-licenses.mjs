import {execFileSync} from 'node:child_process';
import {readFileSync,writeFileSync,readdirSync,statSync} from 'node:fs';
import {dirname,join} from 'node:path';
const metadata=JSON.parse(execFileSync('cargo',['metadata','--locked','--format-version','1'],{encoding:'utf8',maxBuffer:16*1024*1024}));
const packages=metadata.packages.filter(p=>p.source?.startsWith('registry+')).sort((a,b)=>a.name.localeCompare(b.name)||a.version.localeCompare(b.version));
const blocks=['Rust dependency licenses and notices\nSource: Cargo.lock and installed registry package manifests.\nIncludes target-specific and build/test dependencies.'];
for(const p of packages){
 const root=dirname(p.manifest_path);
 const names=readdirSync(root).filter(n=>/^(LICENSE|LICENCE|COPYING|NOTICE)([._-]|$)/i.test(n));
 if(p.license_file && !names.includes(p.license_file))names.push(p.license_file);
 const text=[p.name+' '+p.version,'License: '+(p.license||'see attached license'),'Repository: '+(p.repository||''),...names.filter(n=>statSync(join(root,n)).isFile()).map(n=>n+'\n'+readFileSync(join(root,n),'utf8'))];
 blocks.push(text.join('\n'));
}
writeFileSync('licenses/rust-dependencies.txt',blocks.join('\n\n'+'='.repeat(72)+'\n\n')+'\n');
console.log('Collected notices for '+packages.length+' Rust registry packages');
