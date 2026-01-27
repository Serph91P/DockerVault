import { useState } from 'react';
import { Shield, User, Lock, Check, AlertCircle, ArrowRight } from 'lucide-react';
import { useAuthStore } from '../store/auth';
import toast from 'react-hot-toast';

export default function SetupWizard() {
  const [step, setStep] = useState(1);
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState('');
  
  const setup = useAuthStore((state) => state.setup);
  
  const validateStep1 = () => {
    if (username.length < 3) {
      setError('Username must be at least 3 characters');
      return false;
    }
    setError('');
    return true;
  };
  
  const validateStep2 = () => {
    if (password.length < 8) {
      setError('Password must be at least 8 characters');
      return false;
    }
    if (password !== confirmPassword) {
      setError('Passwords do not match');
      return false;
    }
    setError('');
    return true;
  };
  
  const handleNext = () => {
    if (step === 1 && validateStep1()) {
      setStep(2);
    }
  };
  
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    
    if (!validateStep2()) {
      return;
    }
    
    setIsLoading(true);
    setError('');
    
    try {
      await setup(username, password);
      toast.success('Setup completed! Welcome to DockerVault');
    } catch (err: unknown) {
      const error = err as { response?: { data?: { detail?: string } } };
      setError(error.response?.data?.detail || 'Setup failed');
    } finally {
      setIsLoading(false);
    }
  };
  
  return (
    <div className="min-h-screen bg-dark-900 flex items-center justify-center p-4">
      <div className="w-full max-w-lg">
        {/* Header */}
        <div className="text-center mb-8">
          <div className="inline-flex items-center justify-center w-20 h-20 bg-indigo-500/10 rounded-2xl mb-4">
            <Shield className="w-10 h-10 text-indigo-400" />
          </div>
          <h1 className="text-3xl font-bold text-dark-100">Welcome to DockerVault</h1>
          <p className="text-dark-400 mt-2">Let's set up your admin account</p>
        </div>
        
        {/* Progress Steps */}
        <div className="flex items-center justify-center gap-4 mb-8">
          <div className={`flex items-center gap-2 ${step >= 1 ? 'text-indigo-400' : 'text-dark-500'}`}>
            <div className={`w-8 h-8 rounded-full flex items-center justify-center ${
              step > 1 ? 'bg-indigo-500 text-white' : step === 1 ? 'bg-indigo-500/20 text-indigo-400' : 'bg-dark-700'
            }`}>
              {step > 1 ? <Check className="w-4 h-4" /> : '1'}
            </div>
            <span className="text-sm font-medium">Username</span>
          </div>
          
          <div className="w-12 h-0.5 bg-dark-700">
            <div className={`h-full bg-indigo-500 transition-all ${step > 1 ? 'w-full' : 'w-0'}`} />
          </div>
          
          <div className={`flex items-center gap-2 ${step >= 2 ? 'text-indigo-400' : 'text-dark-500'}`}>
            <div className={`w-8 h-8 rounded-full flex items-center justify-center ${
              step === 2 ? 'bg-indigo-500/20 text-indigo-400' : 'bg-dark-700'
            }`}>
              2
            </div>
            <span className="text-sm font-medium">Password</span>
          </div>
        </div>
        
        {/* Form */}
        <form onSubmit={step === 2 ? handleSubmit : (e) => { e.preventDefault(); handleNext(); }} 
              className="bg-dark-800 rounded-xl border border-dark-700 p-6 space-y-6">
          
          {error && (
            <div className="flex items-center gap-2 p-3 bg-red-500/10 border border-red-500/30 rounded-lg text-red-400 text-sm">
              <AlertCircle className="w-4 h-4 flex-shrink-0" />
              <span>{error}</span>
            </div>
          )}
          
          {step === 1 && (
            <>
              <div className="text-center mb-4">
                <h2 className="text-lg font-semibold text-dark-100">Create Admin Account</h2>
                <p className="text-dark-400 text-sm mt-1">
                  Choose a username for your administrator account
                </p>
              </div>
              
              <div>
                <label className="block text-sm font-medium text-dark-300 mb-2">
                  Username
                </label>
                <div className="relative">
                  <User className="absolute left-3 top-1/2 -translate-y-1/2 w-5 h-5 text-dark-500" />
                  <input
                    type="text"
                    value={username}
                    onChange={(e) => setUsername(e.target.value)}
                    className="w-full pl-10 pr-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 placeholder-dark-500 focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent"
                    placeholder="Enter username (min. 3 characters)"
                    required
                    autoComplete="username"
                    autoFocus
                  />
                </div>
              </div>
              
              <button
                type="submit"
                className="w-full flex items-center justify-center gap-2 py-3 bg-indigo-600 hover:bg-indigo-700 text-white rounded-lg font-medium transition-colors"
              >
                Continue
                <ArrowRight className="w-5 h-5" />
              </button>
            </>
          )}
          
          {step === 2 && (
            <>
              <div className="text-center mb-4">
                <h2 className="text-lg font-semibold text-dark-100">Set Your Password</h2>
                <p className="text-dark-400 text-sm mt-1">
                  Choose a strong password for <span className="text-indigo-400">{username}</span>
                </p>
              </div>
              
              <div>
                <label className="block text-sm font-medium text-dark-300 mb-2">
                  Password
                </label>
                <div className="relative">
                  <Lock className="absolute left-3 top-1/2 -translate-y-1/2 w-5 h-5 text-dark-500" />
                  <input
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    className="w-full pl-10 pr-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 placeholder-dark-500 focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent"
                    placeholder="Enter password (min. 8 characters)"
                    required
                    autoComplete="new-password"
                    autoFocus
                  />
                </div>
              </div>
              
              <div>
                <label className="block text-sm font-medium text-dark-300 mb-2">
                  Confirm Password
                </label>
                <div className="relative">
                  <Lock className="absolute left-3 top-1/2 -translate-y-1/2 w-5 h-5 text-dark-500" />
                  <input
                    type="password"
                    value={confirmPassword}
                    onChange={(e) => setConfirmPassword(e.target.value)}
                    className="w-full pl-10 pr-4 py-3 bg-dark-900 border border-dark-600 rounded-lg text-dark-100 placeholder-dark-500 focus:outline-none focus:ring-2 focus:ring-indigo-500 focus:border-transparent"
                    placeholder="Confirm your password"
                    required
                    autoComplete="new-password"
                  />
                </div>
              </div>
              
              <div className="flex gap-3">
                <button
                  type="button"
                  onClick={() => setStep(1)}
                  className="flex-1 py-3 bg-dark-700 hover:bg-dark-600 text-dark-200 rounded-lg font-medium transition-colors"
                >
                  Back
                </button>
                <button
                  type="submit"
                  disabled={isLoading}
                  className="flex-1 flex items-center justify-center gap-2 py-3 bg-indigo-600 hover:bg-indigo-700 disabled:bg-indigo-600/50 text-white rounded-lg font-medium transition-colors"
                >
                  {isLoading ? (
                    <div className="w-5 h-5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  ) : (
                    <>
                      <Check className="w-5 h-5" />
                      Complete Setup
                    </>
                  )}
                </button>
              </div>
            </>
          )}
        </form>
        
        {/* Info */}
        <div className="mt-6 p-4 bg-dark-800/50 border border-dark-700 rounded-lg">
          <p className="text-dark-400 text-sm text-center">
            This account will have full administrator access to DockerVault.
            Make sure to use a strong password and keep it secure.
          </p>
        </div>
      </div>
    </div>
  );
}
